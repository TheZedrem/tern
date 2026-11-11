# Tern shell integration for zsh: reports the working directory with
# OSC 7, so splits and restored tabs open where you are,
# and an SSH pane's SFTP sidebar opens in the folder you're in on the server.
# Opt-in. Add to ~/.zshrc:  source /path/to/tern.zsh

__tern_osc7() {
    emulate -L zsh
    setopt extendedglob
    # Byte-wise, so non-ASCII names are percent-encoded as UTF-8.
    local LC_ALL=C
    local encoded=${PWD//(#m)[^-\/._~a-zA-Z0-9]/%${(l:2::0:)$(( [##16] #MATCH ))}}
    printf '\e]7;file://%s%s\e\' "$HOST" "$encoded"
}

autoload -Uz add-zsh-hook
add-zsh-hook precmd __tern_osc7
