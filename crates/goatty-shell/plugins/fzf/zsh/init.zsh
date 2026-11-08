# goatty's built-in fzf plugin: key bindings. They need the line editor;
# without a terminal, fzf's script also fails restoring the `zle` option it
# saved.
if [[ -o zle && -t 0 ]] && _goatty_fzf=$(fzf --zsh 2>/dev/null) && [[ -n $_goatty_fzf ]]; then
  eval "$_goatty_fzf"
fi
builtin unset _goatty_fzf
