# goatty's built-in fzf plugin: goatty's options file (colors from the
# theme, layout), fd as the file list, previews, and fzf's key bindings:
# Ctrl+R history, Ctrl+T files, Alt+C directories.
#
# What you set yourself wins: FZF_* variables set before goatty's plugins
# load (your .zshenv, .zprofile) are kept, and ~/.config/goatty/zshrc runs
# after this file anyway.

# _goatty_fzf_export NAME VALUE: export NAME unless you set it yourself.
# What goatty exported is remembered in _goatty_fzf_NAME, so goatty-reload
# (whose new shell inherits it) replaces it rather than keeping it as yours.
_goatty_fzf_export() {
  local mine=_goatty_fzf_$1
  if [[ -z ${(P)1+set} || ${(P)1} == "${(P)mine-}" ]]; then
    builtin export $1=$2 $mine=$2
  fi
}

# _goatty_fzf_unexport NAME: unset NAME if it's what goatty exported.
_goatty_fzf_unexport() {
  local mine=_goatty_fzf_$1
  if [[ -n ${(P)mine+set} && ${(P)1-} == "${(P)mine}" ]]; then
    builtin unset $1 $mine
  fi
}

# _goatty_fzf_helper NAME: record in goatty-status whether an optional
# helper is there (REPLY: its command, or empty).
_goatty_fzf_helper() {
  REPLY=
  local c
  for c in "$@"; do
    (( $+commands[$c] )) && { REPLY=$c; break }
  done
  if [[ -n $REPLY ]]; then
    _goatty_state[helper:$1]="ok"$'\t'"$commands[$REPLY]"
  else
    _goatty_state[helper:$1]=missing
  fi
}

() {
  _goatty_fzf_export FZF_DEFAULT_OPTS_FILE "$goatty_plugin_dir/fzfrc"

  local hidden=--hidden walk=,hidden
  [[ $goatty_settings[hidden_files] == true ]] || hidden= walk=

  # Files and directories: fd (fdfind on Debian and Ubuntu) when it's there,
  # else fzf walks the directory itself with the same rules.
  local fd
  local -a ctrl_t_opts alt_c_opts
  _goatty_fzf_helper fd fdfind
  fd=$REPLY
  if [[ -n $fd ]]; then
    local base="$fd --strip-cwd-prefix${hidden:+ $hidden} --follow --exclude .git"
    _goatty_fzf_export FZF_DEFAULT_COMMAND "$base --type f"
    (( ${+FZF_CTRL_T_COMMAND} )) || typeset -g FZF_CTRL_T_COMMAND="$base --type f"
    (( ${+FZF_ALT_C_COMMAND} )) || typeset -g FZF_ALT_C_COMMAND="$base --type d"
  else
    _goatty_fzf_unexport FZF_DEFAULT_COMMAND
    ctrl_t_opts+=("--walker=file,follow$walk" --walker-skip=.git)
    alt_c_opts+=("--walker=dir,follow$walk" --walker-skip=.git)
  fi

  # Previews: the file under the cursor (bat) and directory contents (eza),
  # in the terminal's colors. Each needs its tool.
  if [[ $goatty_settings[previews] == true ]]; then
    _goatty_fzf_helper bat batcat
    [[ -n $REPLY ]] && ctrl_t_opts+=("--preview=$REPLY --color=always --style=numbers --line-range=:200 --theme=\${BAT_THEME:-ansi} -- {}")
    _goatty_fzf_helper eza
    [[ -n $REPLY ]] && alt_c_opts+=("--preview=eza -1 --icons=always --color=always --group-directories-first -- {}")
  fi
  (( ${+FZF_CTRL_T_OPTS} || ! $#ctrl_t_opts )) || typeset -g FZF_CTRL_T_OPTS=${(j: :)${(qq)ctrl_t_opts}}
  (( ${+FZF_ALT_C_OPTS} || ! $#alt_c_opts )) || typeset -g FZF_ALT_C_OPTS=${(j: :)${(qq)alt_c_opts}}
}

# The key bindings need the line editor; without a terminal, fzf's script
# also fails restoring the `zle` option it saved.
if [[ -o zle && -t 0 ]] && _goatty_fzf=$(fzf --zsh 2>/dev/null) && [[ -n $_goatty_fzf ]]; then
  eval "$_goatty_fzf"
fi
builtin unset _goatty_fzf
builtin unfunction _goatty_fzf_export _goatty_fzf_unexport _goatty_fzf_helper
