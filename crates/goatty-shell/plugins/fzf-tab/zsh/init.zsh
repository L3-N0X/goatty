# goatty's built-in fzf-tab plugin: completion styles and previews, read
# before fzf-tab itself is sourced. fzf reads goatty's options file (the
# fzf plugin's), so the menu has the theme's colors and the preview layout.
# zstyles you set yourself in ~/.config/goatty/zshrc come later and win.

# fzf-tab takes over the menu: no zsh menu, no group headers or colored
# group markers, / accepts a directory and completes inside it.
zstyle ':completion:*' menu no
zstyle ':completion:*:git-checkout:*' sort false
zstyle ':fzf-tab:*' prefix ''
zstyle ':fzf-tab:*' show-group none
zstyle ':fzf-tab:*' single-group ''
zstyle ':fzf-tab:*' group-colors ''
zstyle ':fzf-tab:*' default-color ''
zstyle ':fzf-tab:*' continuous-trigger '/'
# Your own FZF_DEFAULT_OPTS apply here too.
zstyle ':fzf-tab:*' use-fzf-default-opts yes

if [[ $goatty_settings[previews] == true ]]; then
  () {
    # Directories with eza, files with bat (batcat on Debian and Ubuntu), in
    # the terminal's colors. Each needs its tool; without both, paths get
    # no preview.
    local c dir file
    if (( $+commands[eza] )); then
      dir='eza -1 --icons=always --color=always --group-directories-first -- $realpath'
      _goatty_state[helper:eza]="ok"$'\t'"$commands[eza]"
    else
      _goatty_state[helper:eza]=missing
    fi
    for c in bat batcat; do
      (( $+commands[$c] )) || continue
      file="$c --color=always --style=numbers --line-range=:200 --theme=\${BAT_THEME:-ansi} -- \$realpath 2>/dev/null"
      _goatty_state[helper:bat]="ok"$'\t'"$commands[$c]"
      break
    done
    [[ -n $file ]] || _goatty_state[helper:bat]=missing
    if [[ -n $dir$file ]]; then
      zstyle ':fzf-tab:complete:*:*' fzf-preview "
        if [[ -d \$realpath ]]; then ${dir:-:}
        elif [[ -f \$realpath ]]; then ${file:-:}
        fi"
    fi
    [[ -n $dir ]] && zstyle ':fzf-tab:complete:(cd|__zoxide_z):*' fzf-preview $dir
  }

  # Options and the first word of a command: nothing worth previewing.
  zstyle ':fzf-tab:complete:*:options' fzf-preview
  zstyle ':fzf-tab:complete:*:argument-1' fzf-preview

  # kill, ps: the process's whole command line.
  zstyle ':completion:*:*:*:*:processes' command "ps -u $USER -o pid,user,comm -w -w"
  zstyle ':fzf-tab:complete:(kill|ps):argument-rest' fzf-preview 'ps -p $word -o args= -ww 2>/dev/null'
  zstyle ':fzf-tab:complete:(kill|ps):argument-rest' fzf-flags --preview-window=down:3:wrap

  # Variables: their value.
  zstyle ':fzf-tab:complete:(-command-|-parameter-|-brace-parameter-|export|unset|expand):*' fzf-preview 'print -r -- ${(P)word}'

  # git: the diff of a changed file, the last commit of a branch or tag.
  zstyle ':fzf-tab:complete:git-(add|diff|restore):*' fzf-preview 'git diff --color=always -- $word 2>/dev/null'
  zstyle ':fzf-tab:complete:git-log:*' fzf-preview 'git log --color=always -1 $word 2>/dev/null'
  zstyle ':fzf-tab:complete:git-(checkout|switch):*' fzf-preview \
    'git log --color=always -1 $word 2>/dev/null || git show --color=always $word 2>/dev/null'
  zstyle ':fzf-tab:complete:git-show:*' fzf-preview 'git show --color=always $word 2>/dev/null'

  # Services, tldr pages, man pages.
  zstyle ':fzf-tab:complete:systemctl-*:*' fzf-preview 'SYSTEMD_COLORS=1 systemctl status -- $word 2>/dev/null'
  zstyle ':fzf-tab:complete:tldr:argument-1' fzf-preview 'tldr --color=always $word 2>/dev/null'
  zstyle ':fzf-tab:complete:(\\|*/|)man:*' fzf-preview 'MANWIDTH=$FZF_PREVIEW_COLUMNS man $word 2>/dev/null'
fi
