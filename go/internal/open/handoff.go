package open

import (
	"fmt"
	"html"
	"net/http"
	"path/filepath"
	"regexp"
	"strconv"
	"strings"
	"time"

	"github.com/lftherios/session-link/internal/cli"
	"github.com/lftherios/session-link/internal/handoff"
)

func localAction(r *http.Request) bool {
	return r.Header.Get("x-slink") == "1" && (r.Header.Get("Origin") == "" || r.Header.Get("Origin") == "http://"+r.Host)
}

func (s *Server) previewDir() string {
	if s.PreviewDir != "" {
		return s.PreviewDir
	}
	return filepath.Join(filepath.Dir(s.CaptureDir), "previews")
}

// Signing in from another terminal should unlock publishing after a reload,
// without replacing the preview or restarting the viewer.
func (s *Server) apiKey() string {
	target, key := cli.ResolveTarget("", "")
	if target == s.Target && key != "" {
		return key
	}
	return s.APIKey
}

func (s *Server) prepare(ref string) publishResult {
	i, err := strconv.Atoi(ref)
	if err != nil || i < 0 || i >= len(s.Sources) {
		return publishResult{404, map[string]any{"error": map[string]any{"message": "This session is not in the current list. Restart slink view to refresh it."}}}
	}
	id, err := handoff.Save(s.previewDir(), s.Sources[i])
	if err != nil {
		return publishResult{422, map[string]any{"error": map[string]any{"message": err.Error()}}}
	}
	return publishResult{200, map[string]any{"url": "/p/" + id}}
}

func (s *Server) pickerPage() string {
	now := time.Now()
	rows := make([]listRow, 0, len(s.Sources))
	for i, source := range s.Sources {
		harness := harnessLabel(source.Harness)
		// The title the session page shows: one typed for the session, a title
		// the harness recorded, or the untitled label.
		title, untitled := s.typedSessionTitle(source.Harness, source.ID), false
		if title == "" && meaningfulTitle(source.Name) {
			title = strings.TrimSpace(source.Name)
		}
		if title == "" {
			started := source.Started
			if started.IsZero() {
				started = source.Updated
			}
			title, untitled = "Untitled · "+harness, true
			if !started.IsZero() {
				title += " · " + started.Local().Format("Jan 2, 2006")
			}
		}
		var meta []string
		if !untitled {
			meta = append(meta, html.EscapeString(harness))
		}
		if source.Harness == "hermes" {
			meta = append(meta, "experimental")
		}
		if source.Dir != "" && filepath.Clean(source.Dir) != filepath.Clean(s.Project) {
			meta = append(meta, `<span title="`+html.EscapeString(displayPath(source.Dir))+`">`+html.EscapeString(filepath.Base(source.Dir))+`</span>`)
		}
		if source.ID != "" {
			short := source.ID
			if len(short) > 12 {
				short = short[:8]
			}
			meta = append(meta, `<code title="`+html.EscapeString(source.ID)+`">`+html.EscapeString(short)+`</code>`)
		}
		if prompt := strings.TrimSpace(source.Prompt); prompt != "" && prompt != title {
			meta = append(meta, `<span class="row-preview">`+html.EscapeString(prompt)+`</span>`)
		}
		titleClass := "row-title"
		if untitled {
			titleClass += " untitled"
		}
		search := strings.ToLower(strings.Join([]string{title, source.Prompt, harness, source.Harness, source.ID, source.Dir}, " "))
		rows = append(rows, listRow{day: dayLabel(source.Updated, now), html: fmt.Sprintf(
			`<button class="row" data-source="%d" data-search="%s"><span class="%s">%s</span><span class="row-meta">%s</span><span class="row-side">%s<span class="row-open">%s</span></span></button>`,
			i, html.EscapeString(search), titleClass, html.EscapeString(title), strings.Join(meta, listDot), rowStamp(source.Updated, now), iconChevron)})
	}
	name := filepath.Base(filepath.Clean(s.Project))
	warning := ""
	if s.Warning != "" {
		warning = `<p class="list-status err">Some sessions could not be listed: ` + html.EscapeString(s.Warning) + `</p>`
	}
	list := `<div class="empty-state"><strong>No sessions for this project yet</strong>Open a transcript with <code>slink view --session &lt;path&gt;</code>, or record new work with <code>slink record -- &lt;command&gt;</code>.</div>`
	if len(rows) > 0 {
		list = `<label class="find">` + iconSearch + `<input type="search" id="search" aria-label="Find a session" placeholder="Search ` + plural(len(rows), "session", "sessions") + `…" autocomplete="off"></label>
    <p id="result" class="list-status" role="status"></p>` + warning + `<div class="groups">` + groupRows(rows) + `</div>
    <p id="no-match" class="list-status empty-match" hidden>No sessions match this search.</p>`
	} else {
		list = `<p id="result" class="list-status" role="status"></p>` + warning + list
	}
	return page(name+" · session.link", listCSS+`
    <header class="list-head"><h1 title="`+html.EscapeString(displayPath(s.Project))+`">`+html.EscapeString(name)+`</h1><button class="quiet" id="stop">`+iconPower+`Stop viewer</button></header>
    `+list+listScript+`
    <script>
      const result=document.getElementById('result');
      document.querySelectorAll('[data-source]').forEach(row=>row.onclick=async()=>{
        if(row.getAttribute('aria-busy')==='true')return;
        row.setAttribute('aria-busy','true');result.className='list-status';result.textContent='Opening…';
        try{
          const r=await fetch('/api/preview/'+row.dataset.source,{method:'POST',headers:{'x-slink':'1'}});
          const d=await r.json();if(!r.ok)throw new Error(d.error?.message||'Could not open this session.');
          location.assign(d.url);
        }catch(e){result.className='list-status err';result.textContent=e.message;row.removeAttribute('aria-busy');}
      });
      document.getElementById('stop').onclick=async()=>{
        try{const r=await fetch('/api/stop',{method:'POST',headers:{'x-slink':'1'}});if(!r.ok)throw new Error();
          document.querySelector('.wrap').innerHTML='<div class="empty-state stopped"><strong>Viewer stopped</strong>Your saved previews are still on disk. Run <code>slink view</code> to open it again.</div>';
        }catch{result.className='list-status err';result.textContent='Could not stop the viewer. Press Ctrl-C in the terminal that started it.';}
      };
    </script>`)
}

// listRow is one entry in a session list, grouped under its local day.
type listRow struct {
	day  string
	html string
}

const listDot = `<span aria-hidden="true">·</span>`

// Icons follow the viewer's line style.
const (
	iconSearch  = `<svg aria-hidden="true" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="11" cy="11" r="8"/><path d="m21 21-4.3-4.3"/></svg>`
	iconChevron = `<svg aria-hidden="true" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="m9 18 6-6-6-6"/></svg>`
	iconPower   = `<svg aria-hidden="true" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 2v10"/><path d="M18.4 6.6a9 9 0 1 1-12.77.04"/></svg>`
)

// listCSS gives the session lists the viewer's quiet header, search field
// and grouped rows, on the shared tokens.
const listCSS = `<style>
  :root{--soft:#eef1ec}
  @media(prefers-color-scheme:dark){:root{--soft:#1e2620}}
  .wrap{max-width:860px}
  .list-head{display:flex;align-items:center;justify-content:space-between;gap:16px;margin:10px 0 28px}
  .list-head h1{margin:0;min-width:0;font:500 32px/1.2 var(--serif);letter-spacing:-.02em;overflow-wrap:anywhere}
  .quiet{display:inline-flex;align-items:center;gap:7px;flex:none;height:34px;padding:0 12px;border:1px solid var(--line);border-radius:9px;background:var(--panel);color:var(--faint);font:13px system-ui,sans-serif;cursor:pointer}
  .quiet:hover{color:var(--ink)}.quiet:focus-visible,.find:focus-within{outline:none;border-color:var(--signal);box-shadow:0 0 0 2px color-mix(in srgb,var(--signal) 14%,transparent)}
  .find{display:flex;align-items:center;gap:12px;padding:0 16px;border:1px solid var(--line);border-radius:12px;background:var(--panel);color:var(--faint);box-shadow:0 3px 12px #00000005}
  .find input{flex:1;min-width:0;padding:15px 0;border:0;outline:none;background:transparent;color:var(--ink);font:15px system-ui,sans-serif}
  .groups{margin-top:28px}.group{margin:0 0 28px}.group[hidden],.row[hidden]{display:none}
  .group h2{margin:0 0 10px;font:400 11px var(--mono);letter-spacing:.07em;text-transform:uppercase;color:var(--faint)}
  .rows{display:flex;flex-direction:column;gap:10px}
  .row{display:grid;grid-template-columns:minmax(0,1fr) auto;gap:5px 20px;width:100%;padding:16px 18px;border:1px solid var(--line);border-radius:12px;background:var(--panel);color:var(--ink);font:inherit;text-align:left;text-decoration:none;cursor:pointer;transition:border-color .15s,background .15s}
  .row:hover{background:var(--soft);border-color:color-mix(in srgb,var(--signal) 22%,var(--line))}.row:focus-visible{outline:2px solid var(--signal);outline-offset:2px}
  .row-title{grid-column:1;display:-webkit-box;overflow:hidden;-webkit-box-orient:vertical;-webkit-line-clamp:2;font-size:15px;line-height:1.45;overflow-wrap:anywhere}
  .row-meta{grid-column:1;display:flex;align-items:baseline;gap:0 7px;min-width:0;overflow:hidden;white-space:nowrap;font-size:12px;color:var(--faint)}.row-meta>*{flex:none}.row-meta>.row-preview{flex:1 1 auto;min-width:0;overflow:hidden;text-overflow:ellipsis}
  .row-title.untitled{color:var(--faint)}
  .row-meta code{font:11px var(--mono)}.row-meta .live{color:var(--signal)}
  .row-side{grid-column:2;grid-row:1/span 2;display:flex;align-items:center;gap:10px;color:var(--faint);font:12px var(--mono);white-space:nowrap}
  .row-open{display:inline-flex;transition:transform .15s,color .15s}
  .row:hover .row-open,.row:focus-visible .row-open{color:var(--signal);transform:translateX(2px)}
  .row[aria-busy=true]{opacity:.6;cursor:wait}.row.dead{cursor:default;color:var(--faint)}.row.dead:hover{background:transparent;border-color:var(--line)}
  .list-status{margin:12px 2px 0;font:12px var(--mono);color:var(--faint)}.list-status:empty{display:none}.list-status.err{color:var(--error)}.empty-match{margin-top:28px}
  .empty-state{margin-top:8px;padding:40px 24px;border:1px dashed var(--line);border-radius:12px;color:var(--faint);text-align:center;line-height:1.7}
  .empty-state strong{display:block;margin-bottom:4px;color:var(--ink);font:500 19px var(--serif)}.empty-state code{font:12px var(--mono);color:var(--ink)}.stopped{margin-top:18vh}
  @media(max-width:600px){.wrap{padding:20px 16px 48px}.list-head h1{font-size:26px}.row{padding:14px}.row-side{grid-row:1}}
</style>`

// listScript filters rows as people type and moves between them with the
// arrow keys, starting from the search field.
const listScript = `<script>
  (()=>{
    const rows=[...document.querySelectorAll('.row[data-search]')],groups=[...document.querySelectorAll('.group')],search=document.getElementById('search'),noMatch=document.getElementById('no-match');
    const focusable=()=>rows.filter(r=>!r.hidden&&r.matches('a,button'));
    if(search){
      search.oninput=()=>{const q=search.value.trim().toLowerCase();rows.forEach(r=>r.hidden=!r.dataset.search.includes(q));
        groups.forEach(g=>g.hidden=![...g.querySelectorAll('.row')].some(r=>!r.hidden));if(noMatch)noMatch.hidden=rows.some(r=>!r.hidden);};
      search.onkeydown=e=>{if(e.key==='ArrowDown'){e.preventDefault();focusable()[0]?.focus();}};
    }
    rows.forEach(row=>row.addEventListener('keydown',e=>{
      if(e.key!=='ArrowDown'&&e.key!=='ArrowUp')return;e.preventDefault();
      const visible=focusable(),next=visible[visible.indexOf(row)+(e.key==='ArrowDown'?1:-1)];
      if(next)next.focus();else if(e.key==='ArrowUp')search?.focus();
    }));
  })();
</script>`

// groupRows renders rows under day headings, in the order days first appear.
func groupRows(rows []listRow) string {
	var order []string
	byDay := map[string]*strings.Builder{}
	for _, row := range rows {
		b := byDay[row.day]
		if b == nil {
			b = &strings.Builder{}
			byDay[row.day] = b
			order = append(order, row.day)
		}
		b.WriteString(row.html)
	}
	var out strings.Builder
	for _, day := range order {
		fmt.Fprintf(&out, `<section class="group" aria-label="%[1]s"><h2>%[1]s</h2><div class="rows">%[2]s</div></section>`, html.EscapeString(day), byDay[day].String())
	}
	return out.String()
}

// dayLabel names the local day of t for list headings.
func dayLabel(t, now time.Time) string {
	if t.IsZero() {
		return "Undated"
	}
	t, now = t.Local(), now.Local()
	day := time.Date(t.Year(), t.Month(), t.Day(), 0, 0, 0, 0, time.Local)
	today := time.Date(now.Year(), now.Month(), now.Day(), 0, 0, 0, 0, time.Local)
	switch {
	case !day.Before(today):
		return "Today"
	case day.AddDate(0, 0, 1).Equal(today):
		return "Yesterday"
	case day.AddDate(0, 0, 7).After(today):
		return t.Format("Monday")
	case t.Year() == now.Year():
		return t.Format("January 2")
	}
	return t.Format("January 2, 2006")
}

// rowStamp shows how long ago a session from today was active and the time
// of day for older ones; the title carries the full date.
func rowStamp(t, now time.Time) string {
	if t.IsZero() {
		return ""
	}
	label := t.Local().Format("3:04 PM")
	if dayLabel(t, now) == "Today" {
		label = age(t, now)
	}
	return `<time datetime="` + t.UTC().Format(time.RFC3339) + `" title="` + html.EscapeString(t.Local().Format("Monday, January 2, 2006 at 3:04 PM")) + `">` + html.EscapeString(label) + `</time>`
}

var placeholderTitle = regexp.MustCompile(`(?i)^(?:untitled(?: session)?|session|[a-f\d-]{24,}|rollout-.*|.*\.(?:jsonl?|spool))$`)

// meaningfulTitle mirrors the viewer: placeholders, file names, paths and
// injected context are not titles.
func meaningfulTitle(name string) bool {
	name = strings.TrimSpace(name)
	return name != "" && !placeholderTitle.MatchString(name) && !strings.HasPrefix(name, "/") && !strings.HasPrefix(name, "<")
}

// harnessLabel is how people name each agent.
func harnessLabel(harness string) string {
	switch harness {
	case "claude-code":
		return "Claude Code"
	case "codex":
		return "Codex"
	case "opencode":
		return "OpenCode"
	case "hermes":
		return "Hermes"
	case "":
		return "Local capture"
	}
	return harness
}

func plural(n int, one, many string) string {
	if n == 1 {
		return "1 " + one
	}
	return fmt.Sprintf("%d %s", n, many)
}
