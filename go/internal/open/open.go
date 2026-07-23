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
	"errors"
	"fmt"
	"html"
	"net"
	"net/http"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strings"
	"time"

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
  .card.dead{border-style:dashed;color:var(--faint);cursor:default}
  .card.dead .t{color:var(--faint)}
  .card.dead:hover{border-color:var(--line);box-shadow:none;transform:none}
  .card.dead:hover .t{color:var(--faint)}
  .hid{font-family:var(--mono);font-size:.72em}
  dialog{background:var(--panel);color:var(--ink);border:1px solid var(--line);border-radius:10px;
    padding:20px 22px;max-width:460px;width:calc(100% - 48px)}
  dialog::backdrop{background:rgba(23,32,28,.35)}
  .kv{display:flex;gap:12px;font-family:var(--mono);font-size:12px;margin:4px 0}
  .kv .k{color:var(--faint);flex:none;width:52px}
  .kv .v{word-break:break-all}
  .dlg-warn{font-family:var(--mono);font-size:12px;color:var(--error);margin:14px 0 0}
  .dlg-actions{display:flex;justify-content:flex-end;gap:10px;margin-top:16px}
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
	listed := map[string]bool{}
	var cards strings.Builder
	for _, c := range captures {
		listed[filepath.Base(c.File)] = true
		id := strings.TrimSuffix(filepath.Base(c.File), ".json")
		name := c.Name
		if name == "" {
			name = id
		}
		meta := []string{nSpans(c.Spans)}
		if len(c.Models) > 0 {
			meta = append(meta, html.EscapeString(strings.Join(c.Models, ", ")))
		}
		if c.CreatedAt != "" {
			meta = append(meta, agoSpan(c.CreatedAt))
		}
		if c.InProgress {
			meta = append(meta, "recording")
		}
		fmt.Fprintf(&cards, `<a class="card" href="/r/%s"><div class="t">%s</div>
        <div class="m">%s</div></a>`,
			id, html.EscapeString(name), strings.Join(meta, " · "))
	}
	// Files the listing had to skip still exist — grey them out rather than
	// let a local file silently vanish from its own index.
	for _, d := range skippedCaptures(s.CaptureDir, listed) {
		fmt.Fprintf(&cards, `<div class="card dead"><div class="t">%s</div>
        <div class="m">%s</div></div>`, html.EscapeString(d.id), d.meta)
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

// deadCard is an index row for a capture the shell cannot render: no link,
// just the id and why.
type deadCard struct {
	id   string
	meta string // pre-rendered HTML
}

// skippedCaptures is every .json in the capture dir that the listing had to
// skip — unreadable, invalid JSON, or not a session document.
func skippedCaptures(dir string, listed map[string]bool) []deadCard {
	entries, _ := os.ReadDir(dir)
	var out []deadCard
	for _, e := range entries {
		name := e.Name()
		if !strings.HasSuffix(name, ".json") || listed[name] {
			continue
		}
		note := "can't render — invalid JSON"
		raw, err := os.ReadFile(filepath.Join(dir, name))
		if err != nil {
			note = "can't render — unreadable file"
		} else if json.Valid(raw) {
			note = "can't render — not a session document"
		}
		meta := html.EscapeString(note)
		if info, ierr := e.Info(); ierr == nil {
			meta += " · " + agoSpan(info.ModTime().Format(time.RFC3339))
		}
		out = append(out, deadCard{id: strings.TrimSuffix(name, ".json"), meta: meta})
	}
	// Filenames embed the timestamp, so name order is time order (newest first).
	sort.Slice(out, func(i, j int) bool { return out[i].id > out[j].id })
	return out
}

// agoSpan renders a timestamp the way a human scans a card — "2h ago" —
// with the exact moment held in the title attribute for hover.
func agoSpan(iso string) string {
	t, err := time.Parse(time.RFC3339, iso)
	if err != nil {
		return html.EscapeString(iso) // unparseable: show what's there
	}
	return `<span title="` + html.EscapeString(iso) + `">` + age(t, time.Now()) + `</span>`
}

// age words how long ago t was: "just now" through "3w ago", then the
// plain date once relative stops meaning anything.
func age(t, now time.Time) string {
	d := now.Sub(t)
	switch {
	case d < -time.Minute:
		return t.Format("2006-01-02") // future stamp: relative would lie
	case d < time.Minute:
		return "just now"
	case d < time.Hour:
		return fmt.Sprintf("%dm ago", int(d.Minutes()))
	case d < 24*time.Hour:
		return fmt.Sprintf("%dh ago", int(d.Hours()))
	case d < 7*24*time.Hour:
		return fmt.Sprintf("%dd ago", int(d.Hours())/24)
	case d < 30*24*time.Hour:
		return fmt.Sprintf("%dw ago", int(d.Hours())/(24*7))
	default:
		return t.Format("2 Jan 2006")
	}
}

func nSpans(n int) string {
	if n == 1 {
		return "1 span"
	}
	return fmt.Sprintf("%d spans", n)
}

// displayPath abbreviates the home directory to ~ where a human reads a
// path — the same convention as the CLI's terminal output.
func displayPath(p string) string {
	if h, err := os.UserHomeDir(); err == nil && strings.HasPrefix(p, h+string(os.PathSeparator)) {
		return "~" + p[len(h):]
	}
	return p
}

// errNotJSON marks a capture that exists on disk but won't parse — the
// shell renders it as a styled error page, never a raw Go error.
var errNotJSON = errors.New("capture is not valid JSON")

func (s *Server) runPage(id string) (string, error) {
	file := filepath.Join(s.CaptureDir, id+".json")
	spool.Assemble(file, spool.AssembleOptions{}) // live session: fresh snapshot
	raw, err := os.ReadFile(file)
	if err != nil {
		return "", err // os.ErrNotExist → the styled 404
	}
	var run map[string]any
	if err := json.Unmarshal(raw, &run); err != nil {
		return "", errNotJSON
	}
	// The index greys non-session JSON as unrenderable — the direct URL
	// must agree, not serve a half-broken viewer over it.
	if sch, _ := run["schema"].(string); sch != "session/v0" && sch != "run/v0" {
		return "", errNotJSON
	}
	name, _ := run["name"].(string)
	if name == "" {
		name = id
	}
	spans, _ := run["spans"].([]any)
	meta, _ := run["metadata"].(map[string]any)
	inProgress, _ := meta["in_progress"].(bool)
	recNote := ""
	if inProgress {
		recNote = `<div class="kv"><span class="k">status</span><span class="v">still recording — a snapshot as of now will be published</span></div>`
	}
	absFile, err := filepath.Abs(file)
	if err != nil {
		absFile = file
	}
	// <-escaping keeps attacker-controlled trace text inert inside the tag.
	runJSON := strings.ReplaceAll(string(mustCompact(raw)), "<", `\u003c`)
	// json.Marshal escapes < > & by default, so this is script-safe as-is.
	// The file path is only ever read by a human in error text — abbreviated.
	pubJSON, _ := json.Marshal(map[string]any{"hasKey": s.APIKey != "", "file": displayPath(absFile)})
	keyNote, btnLabel := "", "Publish"
	if s.APIKey == "" {
		keyNote = " · no API key (slink login)"
		btnLabel = "Sign in to publish"
	}
	return page(name,
		`<div class="top">
       <p class="eyebrow" style="margin:0"><a href="/">← captures</a> · local preview</p>
       <div class="pub">
         <button class="btn" id="copy" title="copy the page URL, span selection included">copy link</button>
         <button class="btn" id="dl" title="save the run document as `+id+`.json">download JSON</button>
         <span class="note">unlisted → `+html.EscapeString(s.Target)+keyNote+`</span>
         <button class="btn primary" id="pub">`+btnLabel+`</button>
       </div>
     </div>
     <div class="result-row"><span class="result" id="out"></span></div>
     <dialog id="confirm">
       <p class="eyebrow" style="margin:0 0 12px">Publish this capture?</p>
       <div class="kv"><span class="k">server</span><span class="v">`+html.EscapeString(s.Target)+`</span></div>
       <div class="kv"><span class="k">title</span><span class="v">`+html.EscapeString(name)+`</span></div>
       <div class="kv"><span class="k">spans</span><span class="v">`+fmt.Sprintf("%d", len(spans))+`</span></div>
       <div class="kv"><span class="k">size</span><span class="v">~`+approxSize(len(raw))+`</span></div>`+recNote+`
       <p class="dlg-warn">Unlisted is not private — anyone with the link can view it.</p>
       <div class="dlg-actions">
         <button class="btn" id="cancel">Cancel</button>
         <button class="btn primary" id="go">Publish</button>
       </div>
     </dialog>
     <div id="root"></div>
     <script>window.__RUN__=`+runJSON+`</script>
     <script>window.__PUB__=`+string(pubJSON)+`</script>
     <script src="/assets/viewer.js"></script>
     <script>
       const PUB=window.__PUB__,btn=document.getElementById("pub"),out=document.getElementById("out"),
             dlg=document.getElementById("confirm");
       const LOGIN="Not signed in — run `+"`slink login`"+` in a terminal, then reload this page.";
       const hits=d=>d.error?.details?"\n"+d.error.details.map(h=>"  "+(h.pattern??h)+"  "+(h.preview??"")).join("\n"):"";
       if(!PUB.hasKey){
         btn.onclick=()=>{out.className="result err";out.textContent=LOGIN};
       }else{
         btn.onclick=()=>{out.className="result";out.textContent="";dlg.showModal()};
         document.getElementById("cancel").onclick=()=>dlg.close();
         document.getElementById("go").onclick=async()=>{
           dlg.close();
           btn.disabled=true;btn.textContent="Publishing…";
           let res;
           try{
             res=await fetch("/api/publish/`+id+`",{method:"POST",headers:{"x-slink":"1"}});
           }catch(e){
             out.className="result err";
             out.textContent="✗ could not reach the local slink server — is `+"`slink open`"+` still running?";
             btn.disabled=false;btn.textContent="Publish";
             return;
           }
           const d=await res.json().catch(()=>({}));
           if(d.url){
             const url=d.url+(location.hash||"");
             try{await navigator.clipboard.writeText(url)}catch{}
             // DOM APIs, not innerHTML: the URL is server-provided bytes and
             // must never be parsed as markup — nor linked unless it is http(s).
             let link=document.createTextNode(url);
             if(/^https?:\/\//i.test(url)){
               link=document.createElement("a");
               link.href=url;link.target="_blank";link.rel="noopener";link.textContent=url;
             }
             out.replaceChildren(link,(d.deduplicated?"  (already published, copied)":"  (copied)")+" — Anyone with this link can view it.");
             btn.textContent="Published";
           }else{
             out.className="result err";
             if(res.status===401){
               out.textContent=LOGIN;
             }else if(d.error?.code==="secrets_detected"){
               out.textContent="✗ "+(d.error?.message??"publish blocked — credentials detected")+
                 "\nredact the local file: "+(d.error?.path??PUB.file)+hits(d);
             }else{
               out.textContent="✗ "+(d.error?.message??"failed")+hits(d);
             }
             btn.disabled=false;btn.textContent="Publish";
           }
         };
       }
       const copy=document.getElementById("copy"),dl=document.getElementById("dl");
       copy.onclick=async()=>{
         try{await navigator.clipboard.writeText(location.href);copy.textContent="copied"}
         catch{copy.textContent="copy failed"}
         setTimeout(()=>{copy.textContent="copy link"},1200);
       };
       dl.onclick=()=>{
         const u=URL.createObjectURL(new Blob([JSON.stringify(window.__RUN__)],{type:"application/json"}));
         const a=document.createElement("a");
         a.href=u;a.download="`+id+`.json";a.click();
         setTimeout(()=>URL.revokeObjectURL(u),1000);
       };
     </script>`), nil
}

// approxSize renders a byte count the way the dialog wants it: rough, human.
func approxSize(n int) string {
	switch {
	case n < 1024:
		return fmt.Sprintf("%d B", n)
	case n < 1024*1024:
		return fmt.Sprintf("%.1f KB", float64(n)/1024)
	default:
		return fmt.Sprintf("%.1f MB", float64(n)/(1024*1024))
	}
}

func mustCompact(raw []byte) []byte {
	var buf bytes.Buffer
	if err := json.Compact(&buf, raw); err != nil {
		return raw
	}
	return buf.Bytes()
}

// errorShell is the one chrome every error page shares: the way back up
// top, a serif headline, mono detail lines below — same style as the rest
// of the shell.
func errorShell(title, headlineHTML, detailHTML string) string {
	return page(title,
		`<p class="eyebrow" style="margin:0 0 18px"><a href="/">← captures</a> · local preview</p>
     <h1 style="font-family:var(--serif);font-weight:500">`+headlineHTML+`</h1>
     `+detailHTML)
}

// missingPage is the styled 404 for /r/<id> nobody has.
func (s *Server) missingPage(id string) string {
	return errorShell("no session "+id+" here",
		`no session <span class="hid">`+html.EscapeString(id)+`</span> here`,
		`<p class="note">nothing by that id in `+html.EscapeString(displayPath(s.CaptureDir))+`</p>
     <p class="note"><a href="/">back to the captures index</a></p>`)
}

// brokenPage is the styled 500 for a capture that exists but won't render.
func (s *Server) brokenPage(id, why string) string {
	file := filepath.Join(s.CaptureDir, id+".json")
	return errorShell("session "+id+" can't render",
		`session <span class="hid">`+html.EscapeString(id)+`</span> can't render`,
		`<p class="note">`+html.EscapeString(why)+`</p>
     <div class="kv"><span class="k">file</span><span class="v">`+html.EscapeString(displayPath(file))+`</span></div>
     <p class="note"><a href="/">back to the captures index</a></p>`)
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
		abs, aerr := filepath.Abs(file)
		if aerr != nil {
			abs = file
		}
		return publishResult{422, map[string]any{"error": map[string]any{
			"code":    "secrets_detected",
			"message": "publish blocked — credentials detected; redact the local file first",
			"path":    displayPath(abs), // read by a human in the page's error text
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
		switch {
		case errors.Is(err, os.ErrNotExist):
			send(404, "text/html", s.missingPage(id))
		case errors.Is(err, errNotJSON):
			send(500, "text/html", s.brokenPage(id, "the file on disk is not valid JSON"))
		case err != nil:
			send(500, "text/html", s.brokenPage(id, "the file on disk could not be read"))
		default:
			send(200, "text/html", body)
		}
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
		send(404, "text/html", errorShell("no such page",
			`no such page`,
			`<p class="note">everything here starts at <a href="/">the captures index</a></p>`))
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
