# Tern shell integration for bash: reports the working directory with
# OSC 7, so splits and restored tabs open where you are,
# and an SSH pane's SFTP sidebar opens in the folder you're in on the server.
# Opt-in. Add to ~/.bashrc:  . /path/to/tern.bash

__tern_osc7() {
    # Byte-wise, so non-ASCII names are percent-encoded as UTF-8.
    local LC_ALL=C path=$PWD encoded= char i
    for ((i = 0; i < ${#path}; i++)); do
        char=${path:i:1}
        case $char in
            [-/._~a-zA-Z0-9]) encoded+=$char ;;
            *) printf -v char '%%%02X' "'$char"; encoded+=$char ;;
        esac
    done
    printf '\e]7;file://%s%s\e\' "$HOSTNAME" "$encoded"
}

if [[ $PROMPT_COMMAND != *__tern_osc7* ]]; then
    PROMPT_COMMAND="__tern_osc7${PROMPT_COMMAND:+;$PROMPT_COMMAND}"
fi
