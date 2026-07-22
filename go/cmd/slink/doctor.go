package main

import (
	"flag"
	"fmt"
	"net/http"
	"os"
	"time"

	"github.com/lftherios/session-link/internal/cli"
)

// runDoctor checks the recording setup end to end and says what to fix.
// Read-only except for a write-probe in the capture dir; never uploads.
func runDoctor(args []string) {
	fs := flag.NewFlagSet("doctor", flag.ExitOnError)
	setUsage(fs, "slink doctor",
		"Check the recording setup: state dir, tap, this shell's routing,\n  upstreams, and account. Read-only; nothing is uploaded.",
		"slink doctor")
	parseReordered(fs, args)

	fails := 0
	pass := func(ok bool, good, bad string) {
		if ok {
			fmt.Printf("  ✓ %s\n", good)
		} else {
			fails++
			fmt.Printf("  ✗ %s\n", bad)
		}
	}

	// State dir writable — captures die silently without it.
	dir := cli.CaptureDir()
	os.MkdirAll(dir, 0o755)
	probe := dir + "/.doctor-probe"
	werr := os.WriteFile(probe, []byte("ok"), 0o644)
	os.Remove(probe)
	pass(werr == nil, "state dir writable: "+dir, "cannot write "+dir+" — captures will be lost")

	// Tap + routing.
	port := persistedTapPort()
	if port == 0 {
		port = 4141
	}
	info, up := tapInfo(port)
	pass(up, fmt.Sprintf("tap running on :%d", port), fmt.Sprintf("no tap on :%d — always-on capture is off (slink tap, or slink setup); one-off recording still works: slink record -- <cmd>", port))

	routed, routedLive := shellRouting()
	switch {
	case routed:
		pass(routedLive, "this shell routes through the tap", "this shell routes to a tap that isn't running — agents will see connection refused; fix: slink tap, or eval \"$(slink off)\"")
	case customUpstream("ANTHROPIC_BASE_URL") != "" || customUpstream("OPENAI_BASE_URL") != "":
		fmt.Println("  · this shell points at a custom endpoint — slink leaves it alone (eval \"$(slink on)\" to record it)")
	default:
		fmt.Println("  · this shell is not routed — eval \"$(slink on)\" for ambient capture")
	}

	// Upstreams the tap forwards to (misconfigured ones black-hole agents).
	if up {
		client := &http.Client{Timeout: 2 * time.Second}
		for prov, u := range info.Upstreams {
			if u == "https://api.anthropic.com" || u == "https://api.openai.com" {
				continue // vendor defaults — not probed, nothing custom to verify
			}
			// GET, any HTTP status counts: reachability is the question,
			// and API servers routinely reject HEAD or answer 404 on /.
			res, err := client.Get(u)
			if err == nil {
				res.Body.Close()
			}
			pass(err == nil, fmt.Sprintf("%s upstream reachable: %s", prov, u), fmt.Sprintf("%s upstream unreachable: %s — calls through the tap will 502", prov, u))
		}
	}

	// Account — publishing only.
	_, key := cli.ResolveTarget("", "")
	if key == "" {
		fmt.Println("  · not signed in — capture works; publishing will ask (slink share) or: slink login")
	} else if login := cli.ReadConfig().Login; login != "" {
		fmt.Printf("  ✓ signed in as @%s\n", login)
	} else {
		fmt.Println("  ✓ signed in (API key)")
	}

	if fails == 0 {
		fmt.Println("all good — record something: slink record -- <cmd>")
	} else {
		fmt.Println(plural(fails, "problem") + " found — fixes above")
		os.Exit(1)
	}
}
