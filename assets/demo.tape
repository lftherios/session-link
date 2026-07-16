# The slink hero demo. Regenerate from the repo root:  vhs assets/demo.tape
# It records a REAL capture against a mock upstream (assets/demo/) so there are
# no API keys, no network, and the output is exactly what users see.
Output assets/demo.gif

Set Shell zsh
Set FontSize 16
Set Width 1120
Set Height 680
Set Padding 28
Set Margin 16
Set BorderRadius 10
Set TypingSpeed 45ms
Set Theme { "background": "#101613", "foreground": "#dfe6e2", "cursor": "#57c9a6", "selection": "#26312c", "black": "#101613", "red": "#b3402e", "green": "#57c9a6", "yellow": "#c58a3d", "blue": "#5aa0d8", "magenta": "#9a86c4", "cyan": "#57c9a6", "white": "#dfe6e2", "brightBlack": "#8fa39a", "brightRed": "#b3402e", "brightGreen": "#57c9a6", "brightYellow": "#c58a3d", "brightBlue": "#5aa0d8", "brightMagenta": "#9a86c4", "brightCyan": "#57c9a6", "brightWhite": "#ffffff" }

# --- hidden setup: mock backend, clean home, short `slink` alias ---
Hide
Type "setopt interactive_comments" Enter
Type "SERVER=$PWD/assets/demo/server.mjs AGENT=$PWD/assets/demo/agent.mjs" Enter
Type "rm -rf /tmp/slink; node $SERVER & export SLINK_HOME=/tmp/slink SLINK_UPSTREAM_OPENAI=http://127.0.0.1:9099 SLINK_SERVER=http://127.0.0.1:9099" Enter
Type "alias slink='npx --yes session.link'" Enter
Type "cd $(mktemp -d) && cp $AGENT agent.js && slink help >/dev/null 2>&1" Enter
Sleep 2s
Type "clear" Enter
Show

Sleep 800ms
Type "# 1. Record — wrap your agent. Nothing leaves your machine." Sleep 700ms Enter
Type "slink dev -- node agent.js" Sleep 400ms Enter
Sleep 5s

Type "# 2. Publish — validate, secret-scan, one confirm, a permanent link." Sleep 700ms Enter
Type "slink push --yes" Sleep 400ms Enter
Sleep 4s
