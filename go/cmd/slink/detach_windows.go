//go:build windows

package main

import "os/exec"

// Windows daemons aren't shipped yet; Start+Release detaches well enough
// for the CLI convenience path.
func detach(cmd *exec.Cmd) {}
