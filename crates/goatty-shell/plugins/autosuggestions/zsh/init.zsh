# goatty's built-in autosuggestions plugin: options, read before
# zsh-autosuggestions itself is sourced. Values you set yourself win.
if (( ! ${+ZSH_AUTOSUGGEST_STRATEGY} )); then
  case $goatty_settings[strategy] in
    history)          ZSH_AUTOSUGGEST_STRATEGY=(history) ;;
    previous-command) ZSH_AUTOSUGGEST_STRATEGY=(match_prev_cmd completion) ;;
    *)                ZSH_AUTOSUGGEST_STRATEGY=(history completion) ;;
  esac
fi
(( ${+ZSH_AUTOSUGGEST_BUFFER_MAX_SIZE} )) || ZSH_AUTOSUGGEST_BUFFER_MAX_SIZE=$goatty_settings[max_length]
