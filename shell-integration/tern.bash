# Tern shell integration for bash: reports the working directory with
# OSC 7, so splits and restored tabs open where you are,
# and an SSH pane's SFTP sidebar opens in the folder you're in on the server.
# In Tern's local panes it also reports each command you run, for the
# command history suggestions.
# Opt-in. Add to ~/.bashrc:  . /path/to/tern.bash

# Percent-encodes $1 byte-wise into __tern_encoded, so non-ASCII text is
# encoded as UTF-8.
__tern_encode() {
    local LC_ALL=C text=$1 encoded= char i
    for ((i = 0; i < ${#text}; i++)); do
        char=${text:i:1}
        case $char in
            [-/._~a-zA-Z0-9]) encoded+=$char ;;
            *) printf -v char '%%%02X' "'$char"; encoded+=$char ;;
        esac
    done
    __tern_encoded=$encoded
}

__tern_osc7() {
    __tern_encode "$PWD"
    printf '\e]7;file://%s%s\e\' "$HOSTNAME" "$__tern_encoded"
}

# Reports the command bash just added to its history. HISTCMD only moves
# when bash keeps a command, so a plain Enter, or a command HISTCONTROL
# leaves out, reports nothing. The first prompt only notes where the
# history stands: its last entry is from an earlier session.
__tern_history() {
    [[ -n $TERN_HISTORY_TOKEN ]] || return
    if [[ -z $__tern_histcmd ]]; then
        __tern_histcmd=$HISTCMD
        return
    fi
    [[ $HISTCMD == "$__tern_histcmd" ]] && return
    __tern_histcmd=$HISTCMD
    local entry
    entry=$(HISTTIMEFORMAT= builtin history 1)
    [[ $entry =~ ^\ *([0-9]+)[*\ ]\ (.*)$ ]] || return
    __tern_encode "${BASH_REMATCH[2]}"
    printf '\e]6973;command;%s;bash;%s;%s\a' \
        "$TERN_HISTORY_TOKEN" "${BASH_REMATCH[1]}" "$__tern_encoded"
}

# Keeps $? for the prompt commands after it, such as a prompt theme's.
__tern_prompt() {
    local status=$?
    __tern_osc7
    __tern_history
    return $status
}

if [[ $PROMPT_COMMAND != *__tern_prompt* ]]; then
    PROMPT_COMMAND="__tern_prompt${PROMPT_COMMAND:+;$PROMPT_COMMAND}"
fi
