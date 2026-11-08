# goatty's built-in syntax highlighting plugin: options, read before
# zsh-syntax-highlighting itself is sourced. Highlighters you set yourself
# (e.g. in ~/.config/goatty/zshrc) win.
if (( ! ${+ZSH_HIGHLIGHT_HIGHLIGHTERS} )); then
  ZSH_HIGHLIGHT_HIGHLIGHTERS=(main)
  [[ $goatty_settings[brackets] == true ]] && ZSH_HIGHLIGHT_HIGHLIGHTERS+=(brackets)
fi
