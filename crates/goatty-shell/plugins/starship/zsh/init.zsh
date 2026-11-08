# goatty's built-in Starship plugin. goatty generates starship.toml from the
# prompt settings; your own config wins when prefer_user_config is on.
if [[ $goatty_settings[prefer_user_config] != true ]] || [[ -z $STARSHIP_CONFIG && ! -e ${XDG_CONFIG_HOME:-$HOME/.config}/starship.toml ]]; then
  builtin export STARSHIP_CONFIG=$_goatty_dir/starship.toml
fi
eval "$(starship init zsh)"
