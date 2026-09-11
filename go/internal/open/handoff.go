package open

import (
	"fmt"
	"html"
	"net/http"
	"path/filepath"
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
	var cards strings.Builder
	for i, source := range s.Sources {
		harness := source.Harness
		if harness == "" {
			harness = "local capture"
		}
		detail := harness
		if !source.Updated.IsZero() {
			detail += " · " + age(source.Updated, time.Now())
		}
		if source.Dir != "" {
			detail += " · " + displayPath(source.Dir)
		}
		if harness == "hermes" {
			detail += " · experimental"
		}
		fmt.Fprintf(&cards, `<button class="card session" data-source="%d" data-search="%s"><span class="t">%s</span><span class="m">%s</span><span class="sid">%s</span><span class="open-label" aria-hidden="true">Open preview →</span></button>`,
			i, html.EscapeString(strings.ToLower(source.Title+" "+detail+" "+source.ID)), html.EscapeString(source.Title), html.EscapeString(detail), html.EscapeString(source.ID))
	}
	empty := ""
	warning := ""
	if s.Warning != "" {
		warning = `<p class="result err">Some sessions could not be listed: ` + html.EscapeString(s.Warning) + `</p>`
	}
	if len(s.Sources) == 0 {
		empty = `<p class="note">No sessions found for this project yet. Open a specific transcript with <code>slink view --session &lt;path&gt;</code>, or record new work with <code>slink record -- &lt;command&gt;</code>.</p>`
	}
	return page("session.link — choose a session", `<style>
      .intro{max-width:640px;line-height:1.6;color:var(--faint)}
      .session{width:100%;text-align:left;cursor:pointer;font:inherit;position:relative;padding-right:155px}
      .session span{display:block}.session .sid{font:10px var(--mono);color:var(--faint);margin-top:8px;overflow-wrap:anywhere}
      .session .open-label{position:absolute;right:18px;top:20px;color:var(--signal);font:11px var(--mono)}
      .session:focus-visible,.search:focus-visible{outline:2px solid var(--signal);outline-offset:3px}
      .session[hidden]{display:none}.session:disabled{opacity:.6;cursor:wait}
      .search{width:100%;font:14px system-ui;padding:12px 14px;border:1px solid var(--line);border-radius:8px;background:var(--panel);color:var(--ink);margin:12px 0 20px}
      .btn{color:var(--ink)}.empty{font:12px var(--mono);color:var(--faint);padding:24px 0}
      @media(max-width:560px){.wrap{padding:20px 16px}.session{padding-right:18px}.session .open-label{position:static;margin-top:12px}}
    </style>
    <div class="top"><p class="eyebrow">session.link · local sessions</p><button class="btn" id="stop">Stop viewer</button></div>
    <h1 style="font-family:var(--serif);font-weight:500;font-size:32px;margin-bottom:8px">Bring your session to the web.</h1>
    <p class="intro">Choose a session to inspect and share. Opening a preview saves a local snapshot; publishing is a separate step.</p>
    <p class="note">Project: `+html.EscapeString(displayPath(s.Project))+`</p>
    <input class="search" type="search" id="search" aria-label="Find a session" placeholder="Find by task, agent, or session ID…">
    <div id="result" role="status" class="result err"></div>
    `+warning+`<div id="sessions">`+cards.String()+empty+`</div>
    <p id="no-match" class="empty" hidden>No sessions match this search.</p>
    <script>
      const result=document.getElementById('result'),cards=[...document.querySelectorAll('[data-source]')];
      document.getElementById('search').oninput=e=>{
        const q=e.target.value.trim().toLowerCase();
        cards.forEach(card=>card.hidden=!card.dataset.search.includes(q));
        document.getElementById('no-match').hidden=cards.length===0||cards.some(card=>!card.hidden);
      };
      cards.forEach(card=>card.onclick=async()=>{
        card.disabled=true;result.textContent='Opening a local preview…';
        try{
          const r=await fetch('/api/preview/'+card.dataset.source,{method:'POST',headers:{'x-slink':'1'}});
          const d=await r.json();if(!r.ok)throw new Error(d.error?.message||'Could not open this session');
          location.assign(d.url);
        }catch(e){result.textContent=e.message;card.disabled=false;}
      });
      document.getElementById('stop').onclick=async()=>{
        try{const r=await fetch('/api/stop',{method:'POST',headers:{'x-slink':'1'}});if(!r.ok)throw new Error();
          document.body.textContent='Local viewer stopped. Your saved previews are still on disk.';
        }catch{result.textContent='Could not stop the viewer. Use Ctrl-C in the terminal that started it.';}
      };
    </script>`)
}
