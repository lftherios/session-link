// The Go slink CLI — being ported per docs/go-migration.md. Until parity
// holds (scripts/golden.mjs fixtures byte-match), the JS CLI remains the
// reference implementation and the shipped default.
package main

import (
	"fmt"
	"os"
)

var version = "0.0.0-dev" // stamped by goreleaser at P4

func main() {
	if len(os.Args) > 1 && os.Args[1] == "version" {
		fmt.Printf("slink %s (go)\n", version)
		return
	}
	fmt.Fprintln(os.Stderr, "slink(go): port in progress — tap lands first (P1); use the JS CLI meanwhile")
	os.Exit(1)
}
