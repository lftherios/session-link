package main

import (
	"flag"
	"fmt"
	"os"
	"strings"
)

// runCompletion prints a static completion script — commands and their
// flags only; session ids are left to the shell's file completion.
func runCompletion(args []string) {
	fs := flag.NewFlagSet("completion", flag.ExitOnError)
	setUsage(fs, "slink completion <bash|zsh|fish>",
		"Print a tab-completion script. Add the line it suggests to your\n  shell profile.",
		"slink completion zsh >> ~/.zshrc   # or see per-shell hints below")
	parseReordered(fs, args)

	names := strings.Join(commands, " ")
	switch fs.Arg(0) {
	case "bash":
		fmt.Printf(`# slink bash completion — add to ~/.bashrc:  eval "$(slink completion bash)"
_slink() {
  local cur=${COMP_WORDS[COMP_CWORD]}
  if [ "$COMP_CWORD" -eq 1 ]; then
    COMPREPLY=($(compgen -W "%s" -- "$cur"))
  else
    COMPREPLY=($(compgen -f -- "$cur"))
  fi
}
complete -F _slink slink
`, names)
	case "zsh":
		fmt.Printf(`# slink zsh completion — add to ~/.zshrc:  eval "$(slink completion zsh)"
_slink() {
  if (( CURRENT == 2 )); then
    compadd %s
  else
    _files
  fi
}
# compdef exists only after compinit — initialize if the user's rc hasn't.
if ! (( $+functions[compdef] )); then
  autoload -Uz compinit && compinit -i
fi
compdef _slink slink
`, names)
	case "fish":
		fmt.Printf(`# slink fish completion — save as ~/.config/fish/completions/slink.fish
complete -c slink -f -n "__fish_use_subcommand" -a "%s"
complete -c slink -n "not __fish_use_subcommand" -F
`, names)
	default:
		fmt.Fprintln(os.Stderr, "which shell? slink completion <bash|zsh|fish>")
		os.Exit(2)
	}
}
