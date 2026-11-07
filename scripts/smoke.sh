#!/usr/bin/env bash
# Visual smoke test for the renderer: colors, attributes, unicode, wide chars.
# Usage: GOATTY_SHELL=scripts/smoke.sh goatty --local
printf '\e[1mgoatty renderer smoke test\e[0m  (TERM=%s)\n\n' "$TERM"
for i in 0 1 2 3 4 5 6 7; do printf '\e[4%dm  \e[0m' $i; done; echo "  normal bg"
for i in 0 1 2 3 4 5 6 7; do printf '\e[10%dm  \e[0m' $i; done; echo "  bright bg"
for i in 1 2 3 4 5 6 7; do printf '\e[3%dmcolor%d \e[0m' $i $i; done; echo
for i in 1 2 3 4 5 6 7; do printf '\e[1;3%dmbold%d  \e[0m' $i $i; done; echo
for i in $(seq 16 51); do printf '\e[48;5;%dm \e[0m' $i; done; echo "  256-color"
for i in $(seq 0 4 255); do printf '\e[48;2;%d;%d;%dm \e[0m' $i $((255-i)) 128; done; echo
printf '\e[1mbold\e[0m \e[3mitalic\e[0m \e[4munderline\e[0m \e[4:3mcurly\e[0m \e[9mstrike\e[0m \e[2mdim\e[0m \e[7minverse\e[0m\n'
echo 'unicode: héllo wörld  é combining  ─│┌┐└┘├┤ box  ▁▂▃▄▅▆▇█ blocks'
echo 'wide:    日本語テキスト | 한국어 | 中文 | 🦀 🐐 ✨ emoji'
echo 'nerd:      git   folder   rust'
echo 'ligs:    -> => != === <= >= :: |> www'
echo
exec "${SHELL_AFTER:-zsh}" -l
