// Package open is `slink open` — browse local captures in the exact viewer
// the hosted site renders, and publish from the page you're looking at.
// WYSIWYG egress: zero rendering surprise between preview and the shared
// URL. Binds to 127.0.0.1 only; publishing requires a custom header and a
// local Origin so a hostile web page can't drive-by-publish a capture.
package open

import (
	"bytes"
	_ "embed"
	"encoding/json"
	"fmt"
	"html"
	"net"
	"net/http"
	"os"
	"path/filepath"
	"regexp"
	"strings"

	"github.com/lftherios/session-link/internal/cli"
	"github.com/lftherios/session-link/internal/spool"
)

//go:embed viewer.js
var viewerJS []byte

var fileID = regexp.MustCompile(`^[A-Za-z0-9._-]+$`)
var routeRe = regexp.MustCompile(`^/(r|api/publish)/([^/]+)$`)

const css = `
  :root{--paper:#f5f6f3;--panel:#fdfdfb;--ink:#17201c;--faint:#5b6660;--line:#d8ddd7;--signal:#0e6f5c;--error:#b3402e;
    --serif:"Iowan Old Style","Palatino Linotype",Palatino,Georgia,serif;
    --mono:ui-monospace,"SF Mono",Menlo,Consolas,monospace}
  *{box-sizing:border-box}body{margin:0;background:var(--paper);color:var(--ink);
    font-family:system-ui,-apple-system,"Segoe UI",Roboto,sans-serif}
  .wrap{max-width:1080px;margin:0 auto;padding:28px 24px 64px}
  .eyebrow{font-family:var(--mono);font-size:11px;letter-spacing:.14em;text-transform:uppercase;color:var(--faint)}
  a{color:var(--signal)}
  .top{display:flex;justify-content:space-between;align-items:center;gap:12px;flex-wrap:wrap}
  .pub{display:flex;align-items:center;gap:10px}
  .result-row{display:flex;justify-content:flex-end;margin:6px 0 16px}
  .btn{border:1px solid var(--line);background:var(--panel);border-radius:6px;padding:7px 14px;
    font-family:var(--mono);font-size:12px;cursor:pointer;transition:opacity .15s ease}
  .btn.primary{background:var(--signal);border-color:var(--signal);color:#fff}
  .btn:hover{opacity:.88}
  .note{font-family:var(--mono);font-size:12px;color:var(--faint)}
  .result{font-family:var(--mono);font-size:12px}
  .result.err{color:var(--error);white-space:pre-wrap}
  .card{display:block;background:var(--panel);border:1px solid var(--line);border-radius:10px;
    padding:14px 18px;margin-bottom:10px;color:var(--ink);text-decoration:none;
    transition:border-color .15s ease,box-shadow .15s ease,transform .15s ease}
  .card:hover{border-color:#b9c4bb;box-shadow:0 2px 14px rgba(23,32,28,.07);transform:translateY(-1px)}
  .card:hover .t{color:var(--signal)}
  .card .t{font-family:var(--serif);font-size:18px;margin-bottom:3px}
  .card .m{font-family:var(--mono);font-size:12px;color:var(--faint)}
`

const favicon = `<link rel="icon" href='data:image/svg+xml,<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100"><rect width="100" height="100" rx="22" fill="%230e6f5c"/><circle cx="50" cy="50" r="16" fill="%23fdfdfb"/></svg>'>`

func page(title, body string) string {
	return `<!doctype html><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>` + html.EscapeString(title) + `</title>` + favicon + `<style>` + css + `</style><div class="wrap">` + body + `</div>`
}

// Server carries the open UI's state.
type Server struct {
	CaptureDir string
	Target     string // publish server
	APIKey     string
}

func (s *Server) indexPage() string {
	captures := cli.ListCaptures(s.CaptureDir)
	var cards strings.Builder
	for _, c := range captures {
		id := strings.TrimSuffix(filepath.Base(c.File), ".json")
		name := c.Name
		if name == "" {
			name = id
		}
		rec := ""
		if c.InProgress {
			rec = " · recording"
		}
		fmt.Fprintf(&cards, `<a class="card" href="/r/%s"><div class="t">%s</div>
        <div class="m">%d spans · %s · %s%s</div></a>`,
			id, html.EscapeString(name), c.Spans,
			html.EscapeString(strings.Join(c.Models, ", ")), html.EscapeString(c.CreatedAt), rec)
	}
	body := cards.String()
	if body == "" {
		body = `<p class="note">nothing captured yet — try: slink tap</p>`
	}
	return page("session.link — local captures",
		`<p class="eyebrow">session.link · local captures · nothing here has left your machine</p>
     <h1 style="font-family:var(--serif);font-weight:500">Captured sessions</h1>
     `+body)
}

func (s *Server) runPage(id string) (string, error) {
	file := filepath.Join(s.CaptureDir, id+".json")
	spool.Assemble(file, spool.AssembleOptions{}) // live session: fresh snapshot
	raw, err := os.ReadFile(file)
	if err != nil {
		return "", err
	}
	var run map[string]any
	if err := json.Unmarshal(raw, &run); err != nil {
		return "", err
	}
	name, _ := run["name"].(string)
	if name == "" {
		name = id
	}
	// <-escaping keeps attacker-controlled trace text inert inside the tag.
	runJSON := strings.ReplaceAll(string(mustCompact(raw)), "<", `\u003c`)
	keyNote := ""
	if s.APIKey == "" {
		keyNote = " · no API key (slink login)"
	}
	return page(name,
		`<div class="top">
       <p class="eyebrow" style="margin:0"><a href="/">← captures</a> · local preview</p>
       <div class="pub">
         <span class="note">unlisted → `+html.EscapeString(s.Target)+keyNote+`</span>
         <button class="btn primary" id="pub">Publish</button>
       </div>
     </div>
     <div class="result-row"><span class="result" id="out"></span></div>
     <div id="root"></div>
     <script>window.__RUN__=`+runJSON+`</script>
     <script src="/assets/viewer.js"></script>
     <script>
       const btn=document.getElementById("pub"),out=document.getElementById("out");
       btn.onclick=async()=>{
         btn.disabled=true;btn.textContent="Publishing…";out.className="result";out.textContent="";
         const res=await fetch("/api/publish/`+id+`",{method:"POST",headers:{"x-slink":"1"}});
         const d=await res.json();
         if(d.url){
           const url=d.url+(location.hash||"");
           try{await navigator.clipboard.writeText(url)}catch{}
           out.innerHTML='<a href="'+url+'" target="_blank" rel="noopener"></a>';
           out.firstChild.textContent=url;
           out.append(d.deduplicated?"  (already published, copied)":"  (copied)");
           btn.textContent="Published";
         }else{
           out.className="result err";
           out.textContent="✗ "+(d.error?.message??"failed")+(d.error?.details?"\n"+d.error.details.map(h=>"  "+(h.pattern??h)+"  "+(h.preview??"")).join("\n"):"");
           btn.disabled=false;btn.textContent="Publish";
         }
       };
     </script>`), nil
}

func mustCompact(raw []byte) []byte {
	var buf bytes.Buffer
	if err := json.Compact(&buf, raw); err != nil {
		return raw
	}
	return buf.Bytes()
}

type publishResult struct {
	Status int
	Body   map[string]any
}

func (s *Server) publish(id string) publishResult {
	file := filepath.Join(s.CaptureDir, id+".json")
	ins, err := cli.InspectRunFile(file)
	if err != nil {
		return publishResult{400, map[string]any{"error": map[string]any{"message": err.Error()}}}
	}
	if len(ins.Errors) > 0 {
		return publishResult{422, map[string]any{"error": map[string]any{
			"message": "not a valid session/v0 document", "details": ins.Errors,
		}}}
	}
	if len(ins.Secrets) > 0 {
		return publishResult{422, map[string]any{"error": map[string]any{
			"message": "publish blocked — credentials detected; redact the local file first",
			"details": ins.Secrets,
		}}}
	}
	res := cli.UploadRun(ins.Text, s.Target, s.APIKey)
	if !res.OK {
		status := res.Status
		if status == 0 {
			status = 502
		}
		body := res.Body
		if body == nil {
			body = map[string]any{"error": map[string]any{"message": "upload failed"}}
		}
		return publishResult{status, body}
	}
	return publishResult{200, map[string]any{"url": res.Body["url"], "deduplicated": res.Body["deduplicated"]}}
}

func (s *Server) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	send := func(status int, ctype, body string) {
		w.Header().Set("content-type", ctype)
		w.WriteHeader(status)
		w.Write([]byte(body))
	}
	m := routeRe.FindStringSubmatch(r.URL.Path)
	id := ""
	if m != nil && fileID.MatchString(m[2]) {
		id = m[2]
	}
	switch {
	case r.URL.Path == "/":
		send(200, "text/html", s.indexPage())
	case r.URL.Path == "/assets/viewer.js":
		send(200, "text/javascript", string(viewerJS))
	case m != nil && m[1] == "r" && id != "" && r.Method == http.MethodGet:
		body, err := s.runPage(id)
		if err != nil {
			send(500, "text/plain", err.Error())
			return
		}
		send(200, "text/html", body)
	case m != nil && m[1] == "api/publish" && id != "" && r.Method == http.MethodPost:
		// CSRF guard: the custom header forces a CORS preflight (never
		// answered), and the Origin must be this server.
		origin := r.Header.Get("origin")
		if r.Header.Get("x-slink") != "1" ||
			(origin != "" && !strings.HasPrefix(origin, "http://127.0.0.1:") && !strings.HasPrefix(origin, "http://localhost:")) {
			send(403, "application/json", `{"error":{"message":"cross-origin publish blocked"}}`)
			return
		}
		out := s.publish(id)
		b, _ := json.Marshal(out.Body)
		send(out.Status, "application/json", string(b))
	default:
		send(404, "text/plain", "not found")
	}
}

// Serve binds 127.0.0.1 and returns the base URL plus a close func.
func (s *Server) Serve(port int) (string, func(), error) {
	ln, err := net.Listen("tcp", fmt.Sprintf("127.0.0.1:%d", port))
	if err != nil {
		return "", nil, err
	}
	srv := &http.Server{Handler: s}
	go srv.Serve(ln)
	addr := fmt.Sprintf("http://127.0.0.1:%d", ln.Addr().(*net.TCPAddr).Port)
	return addr, func() { srv.Close() }, nil
}
