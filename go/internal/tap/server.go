package tap

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"log"
	"net"
	"net/http"
	"os"
	"regexp"
	"strings"
	"sync"
	"time"

	"github.com/lftherios/session-link/internal/normalize"
	"github.com/lftherios/session-link/internal/spool"
)

// CaptureCap mirrors the JS proxy: stop teeing into memory past this point
// (the client still gets the full stream); the span is marked partial.
const CaptureCap = 32 << 20

var reqDrop = map[string]bool{
	"host": true, "connection": true, "content-length": true,
	"transfer-encoding": true, "accept-encoding": true, "keep-alive": true,
	"upgrade": true, "te": true, "trailer": true, "proxy-authorization": true,
	// slink-internal routing hints — never sent upstream.
	SessionHeader: true, LabelHeader: true,
}

var resDrop = map[string]bool{
	"content-encoding": true, "content-length": true,
	"transfer-encoding": true, "connection": true, "keep-alive": true,
}

var routeRe = regexp.MustCompile(`^/(anthropic|openai)(/.*)$`)

func endpointKind(provider, subpath string) string {
	p := strings.TrimSuffix(strings.SplitN(subpath, "?", 2)[0], "/")
	switch {
	case provider == "anthropic" && p == "/v1/messages":
		return "anthropic.messages"
	case provider == "openai" && strings.HasSuffix(p, "/chat/completions"):
		return "openai.chat"
	case provider == "openai" && strings.HasSuffix(p, "/responses"):
		return "openai.responses"
	}
	return "" // pass through unrecorded
}

// Server is the always-on tap: proxy, tee, segment, capture.
type Server struct {
	CaptureDir string
	Idle       time.Duration
	Upstreams  map[string]string // provider → base URL

	router   *Router
	client   *http.Client
	inflight sync.WaitGroup
}

func NewServer(captureDir string, idle time.Duration) *Server {
	s := &Server{
		CaptureDir: captureDir,
		Idle:       idle,
		Upstreams: map[string]string{
			"anthropic": envOr("SLINK_UPSTREAM_ANTHROPIC", "https://api.anthropic.com"),
			"openai":    envOr("SLINK_UPSTREAM_OPENAI", "https://api.openai.com"),
		},
		// No overall timeout: LLM streams run long; cancellation rides the
		// request context (client disconnect aborts upstream).
		client: &http.Client{},
	}
	s.router = NewRouter(idle, nil,
		func(name string) *Session {
			if name == "" {
				name = "ambient session"
			}
			return NewSession(captureDir, name, time.Now())
		},
		func(sess *Session) { sess.Finalize() },
	)
	return s
}

func envOr(key, def string) string {
	if v := os.Getenv(key); v != "" {
		return v
	}
	return def
}

func (s *Server) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	if r.URL.Path == "/" && r.Method == http.MethodGet {
		w.Header().Set("content-type", "application/json")
		json.NewEncoder(w).Encode(map[string]any{"service": "session.link", "sessions": s.router.Size()})
		return
	}
	m := routeRe.FindStringSubmatch(r.URL.RequestURI())
	if m == nil {
		w.Header().Set("content-type", "application/json")
		w.WriteHeader(http.StatusNotFound)
		json.NewEncoder(w).Encode(map[string]any{"error": map[string]any{
			"code": "unknown_route", "message": "expected /anthropic/* or /openai/*",
		}})
		return
	}
	provider, subpath := m[1], m[2]

	kind := ""
	if r.Method == http.MethodPost {
		kind = endpointKind(provider, subpath)
	}

	reqBuf, err := io.ReadAll(r.Body)
	if err != nil {
		http.Error(w, "read body", http.StatusBadRequest)
		return
	}

	var session *Session
	if kind != "" {
		key := headerOr(r, SessionHeader, AmbientKey)
		name := headerOr(r, LabelHeader, "")
		if name == "" {
			name = deriveName(reqBuf)
		}
		session = s.router.Route(key, name)
		session.Begin()
		defer session.End()
	}
	s.inflight.Add(1)
	defer s.inflight.Done()

	up, err := http.NewRequestWithContext(r.Context(), r.Method, s.Upstreams[provider]+subpath, strings.NewReader(string(reqBuf)))
	if err != nil {
		http.Error(w, err.Error(), http.StatusBadGateway)
		return
	}
	for k, vs := range r.Header {
		if reqDrop[strings.ToLower(k)] {
			continue
		}
		for _, v := range vs {
			up.Header.Add(k, v)
		}
	}

	started := time.Now()
	resp, err := s.client.Do(up)
	if err != nil {
		msg := fmt.Sprintf("upstream unreachable: %v", err)
		w.Header().Set("content-type", "application/json")
		w.WriteHeader(http.StatusBadGateway)
		json.NewEncoder(w).Encode(map[string]any{"error": map[string]any{
			"code": "upstream_unreachable", "message": msg,
		}})
		if session != nil {
			s.recordCall(session, kind, reqBuf, nil, false, started, time.Now(), http.StatusBadGateway, msg, false)
		}
		return
	}
	defer resp.Body.Close()

	for k, vs := range resp.Header {
		if resDrop[strings.ToLower(k)] {
			continue
		}
		for _, v := range vs {
			w.Header().Add(k, v)
		}
	}
	w.WriteHeader(resp.StatusCode)

	isSse := strings.Contains(resp.Header.Get("content-type"), "text/event-stream")
	flusher, _ := w.(http.Flusher)
	var captured []byte
	overflow := false
	transportError := ""
	buf := make([]byte, 32<<10)
	for {
		n, rerr := resp.Body.Read(buf)
		if n > 0 {
			// Blocking writes to the client ARE the backpressure; a gone
			// client errors here while the context abort stops upstream.
			w.Write(buf[:n])
			if flusher != nil && isSse {
				flusher.Flush()
			}
			if len(captured) < CaptureCap {
				captured = append(captured, buf[:n]...)
			} else {
				overflow = true
			}
		}
		if rerr == io.EOF {
			break
		}
		if rerr != nil {
			if r.Context().Err() != nil {
				transportError = "client disconnected mid-stream"
			} else {
				transportError = fmt.Sprintf("upstream stream error: %v", rerr)
			}
			break
		}
	}

	if session != nil {
		s.recordCall(session, kind, reqBuf, captured, isSse, started, time.Now(), resp.StatusCode, transportError, overflow)
	}
}

func (s *Server) recordCall(sess *Session, kind string, reqBuf, resBuf []byte, isSse bool, started, ended time.Time, status int, transportError string, overflow bool) {
	var request any
	json.Unmarshal(reqBuf, &request) // nil on failure, like JS

	var response any
	assembled := true
	if resBuf != nil {
		if isSse {
			events := normalize.ParseSseText(string(resBuf))
			var asm map[string]any
			switch kind {
			case "anthropic.messages":
				asm = normalize.AssembleAnthropicSse(events)
			case "openai.chat":
				asm = normalize.AssembleOpenaiChatSse(events)
			default:
				asm = normalize.AssembleResponsesSse(events)
			}
			if asm == nil {
				assembled = false
				response = map[string]any{"unassembled_sse": truncate(string(resBuf), 100_000)}
			} else {
				response = asm
			}
		} else {
			var parsed any
			if json.Unmarshal(resBuf, &parsed) != nil {
				assembled = false
				response = map[string]any{"unparsed_body": truncate(string(resBuf), 100_000)}
			} else {
				response = parsed
			}
		}
	}
	sess.Record(func(id string) (map[string]any, string) {
		span := normalize.BuildLlmSpan(kind, normalize.BuildContext{
			ID: id, ParentID: "root",
			Request: request, Response: response,
			StartedAt:  started.UTC().Format("2006-01-02T15:04:05.000Z"),
			EndedAt:    ended.UTC().Format("2006-01-02T15:04:05.000Z"),
			HTTPStatus: status, Streamed: isSse,
			CaptureGap:     overflow || !assembled,
			TransportError: transportError,
		})
		endedAt, _ := span["ended_at"].(string)
		return span, endedAt
	})
}

func truncate(s string, n int) string {
	if len(s) > n {
		return s[:n]
	}
	return s
}

func headerOr(r *http.Request, key, def string) string {
	if v := strings.TrimSpace(r.Header.Get(key)); v != "" {
		return v
	}
	return def
}

// deriveName mirrors the JS: the first user message's text, ellipsized,
// else the model id.
func deriveName(reqBuf []byte) string {
	var body struct {
		Model    any `json:"model"`
		Messages []struct {
			Role    string          `json:"role"`
			Content json.RawMessage `json:"content"`
		} `json:"messages"`
	}
	if json.Unmarshal(reqBuf, &body) != nil {
		return ""
	}
	for _, m := range body.Messages {
		if m.Role != "user" {
			continue
		}
		var text string
		var asString string
		if json.Unmarshal(m.Content, &asString) == nil {
			text = asString
		} else {
			var parts []struct {
				Text string `json:"text"`
			}
			if json.Unmarshal(m.Content, &parts) == nil {
				var joined []string
				for _, p := range parts {
					joined = append(joined, p.Text)
				}
				text = strings.Join(joined, " ")
			}
		}
		text = strings.TrimSpace(text)
		if text != "" {
			runes := []rune(text)
			if len(runes) > 60 {
				return string(runes[:57]) + "…"
			}
			return text
		}
		break
	}
	if s, ok := body.Model.(string); ok {
		return s
	}
	return ""
}

// Serve runs the tap until ctx is done: recovery at start, sweep +
// heartbeat tickers, then the protocol shutdown order — stop intake,
// drain in-flight (bounded), finalize all.
func (s *Server) Serve(ctx context.Context, addr string) error {
	if recovered := spool.RecoverDead(s.CaptureDir); recovered > 0 {
		log.Printf("recovered %d interrupted capture(s)", recovered)
	}

	ln, err := net.Listen("tcp", addr)
	if err != nil {
		return err
	}
	srv := &http.Server{Handler: s}

	sweep := time.NewTicker(minDur(s.Idle, time.Minute))
	heartbeat := time.NewTicker(spool.HeartbeatEvery)
	done := make(chan struct{})
	go func() {
		for {
			select {
			case <-sweep.C:
				s.router.Sweep()
			case <-heartbeat.C:
				for _, sess := range s.router.Open() {
					sess.Heartbeat()
				}
			case <-done:
				return
			}
		}
	}()

	port := ln.Addr().(*net.TCPAddr).Port
	log.Printf("session.link tap · http://127.0.0.1:%d · capturing to %s", port, s.CaptureDir)
	log.Printf("export ANTHROPIC_BASE_URL=http://127.0.0.1:%d/anthropic", port)
	log.Printf("export OPENAI_BASE_URL=http://127.0.0.1:%d/openai/v1", port)

	errCh := make(chan error, 1)
	go func() { errCh <- srv.Serve(ln) }()

	select {
	case <-ctx.Done():
	case err := <-errCh:
		close(done)
		return err
	}

	sweep.Stop()
	heartbeat.Stop()
	close(done)
	shutCtx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	srv.Shutdown(shutCtx)
	// Drain in-flight BEFORE finalizing — finalize consumes the spool, and
	// a record landing after that has nowhere valid to go.
	waitTimeout(&s.inflight, 5*time.Second)
	s.router.FinalizeAll()
	return nil
}

func minDur(a, b time.Duration) time.Duration {
	if a < b {
		return a
	}
	return b
}

func waitTimeout(wg *sync.WaitGroup, d time.Duration) bool {
	ch := make(chan struct{})
	go func() {
		wg.Wait()
		close(ch)
	}()
	select {
	case <-ch:
		return true
	case <-time.After(d):
		return false
	}
}
