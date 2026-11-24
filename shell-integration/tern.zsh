# Tern shell integration for zsh: reports the working directory with
# OSC 7, so splits and restored tabs open where you are,
# and an SSH pane's SFTP sidebar opens in the folder you're in on the server.
# In Tern's local panes it also reports each command you run, for the
# command history suggestions.
# Opt-in. Add to ~/.zshrc:  source /path/to/tern.zsh

# Percent-encodes $1 byte-wise, so non-ASCII text is encoded as UTF-8.
__tern_encode() {
    emulate -L zsh
    setopt extendedglob
    local LC_ALL=C
    REPLY=${1//(#m)[^-\/._~a-zA-Z0-9]/%${(l:2::0:)$(( [##16] #MATCH ))}}
}

__tern_osc7() {
    __tern_encode "$PWD"
    printf '\e]7;file://%s%s\e\' "$HOST" "$REPLY"
}

# zshaddhistory hands over the line exactly as typed, before it runs; the
# next prompt reports it. A line starting with a space is left out, as
# HIST_IGNORE_SPACE does.
__tern_addhistory() {
    [[ -n $TERN_HISTORY_TOKEN && $1 != [[:space:]]* ]] && __tern_pending=${1%$'\n'}
    return 0
}

__tern_history() {
    [[ -n $__tern_pending ]] || return
    (( __tern_count++ ))
    __tern_encode "$__tern_pending"
    __tern_pending=
    printf '\e]6973;command;%s;zsh;%s;%s\a' "$TERN_HISTORY_TOKEN" "$__tern_count" "$REPLY"
}

# The last report tells Tern a prompt begins, so it knows where your
# input starts.
__tern_precmd() {
    __tern_osc7
    __tern_history
    [[ -n $TERN_HISTORY_TOKEN ]] &&
        printf '\e]6973;prompt;%s;zsh\a' "$TERN_HISTORY_TOKEN"
    return 0
}

autoload -Uz add-zsh-hook
add-zsh-hook precmd __tern_precmd
add-zsh-hook zshaddhistory __tern_addhistory
