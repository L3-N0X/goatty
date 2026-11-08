# goatty's built-in Atuin plugin.
if [[ $goatty_settings[up_arrow] == true ]]; then
  eval "$(atuin init zsh)"
else
  eval "$(atuin init zsh --disable-up-arrow)"
fi
