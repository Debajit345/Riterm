# riterm shell integration for bash.
#
# Emits iTerm2-compatible OSC 133 prompt marks:
#   A  start of prompt
#   B  end of prompt / start of user input
#   C  command started
#   D  command finished (with exit status)
#
# It also reports the working directory with OSC 7.
#
# This file is part of riterm, licensed GPL-3.0-or-later.

[[ $- == *i* ]] || return 0
[[ -n "${RITERM:-}" ]] || return 0
[[ "${__RITERM_BASH_LOADED:-}" == 1 ]] && return 0
__RITERM_BASH_LOADED=1

__riterm_osc() {
    printf '\033]%s\007' "$1"
}

# Usage: iterm2_set_user_var key value
# The value is base64-encoded as iTerm2 expects. Use it in a badge with
# \(user.key).
iterm2_set_user_var() {
    local encoded
    encoded=$(printf '%s' "$2" | base64 | tr -d '\n')
    printf '\033]1337;SetUserVar=%s=%s\007' "$1" "$encoded"
}

# Usage: riterm_set_badge 'format'
# The format may contain interpolations such as \(session.path). Passing an
# empty string clears the badge.
riterm_set_badge() {
    local encoded
    encoded=$(printf '%s' "$1" | base64 | tr -d '\n')
    printf '\033]1337;SetBadgeFormat=%s\007' "$encoded"
}

# Percent-encode a path for OSC 7. LC_CTYPE=C forces byte-wise iteration so
# each UTF-8 byte is encoded separately, matching the receiver's decoder.
# Stores the result in __RITERM_ENCODED_PATH to avoid forking each prompt.
__riterm_encode_path() {
    local path=$1 out="" ch hex i
    local LC_CTYPE=C LC_COLLATE=C LC_ALL=
    for ((i = 0; i < ${#path}; ++i)); do
        ch="${path:i:1}"
        if [[ "$ch" =~ [/._~A-Za-z0-9-] ]]; then
            out+="$ch"
        else
            # Keep the low two hex digits for bytes above 127.
            printf -v hex "%02X" "'$ch"
            out+="%${hex: -2:2}"
        fi
    done
    __RITERM_ENCODED_PATH="$out"
}

# PROMPT_COMMAND runs before the prompt is drawn and still sees the exit
# status of the command that just finished. Like iTerm2, emit D on every
# prompt (including the first, where it just closes the previous command).
__riterm_precmd() {
    local status=$?
    local host="${HOSTNAME:-localhost}"
    local user="${USER:-user}"
    host="${host//[^A-Za-z0-9._-]/}"
    user="${user//[^A-Za-z0-9._@-]/}"
    __riterm_osc "133;D;${status}"
    __riterm_encode_path "$PWD"
    __riterm_osc "7;file://${user}@${host}${__RITERM_ENCODED_PATH}"
}

case "${PROMPT_COMMAND:-}" in
    *__riterm_precmd*) ;;
    "") PROMPT_COMMAND="__riterm_precmd" ;;
    *) PROMPT_COMMAND="__riterm_precmd;${PROMPT_COMMAND}" ;;
esac

# PS0 is printed after a command line is read and before it executes, so it
# marks the start of command output.
PS0=$'\033]133;C\a'"${PS0:-}"

# Bracket the prompt: A at its start, B at its end. Wrapped in \[ \] so
# readline does not count the escape sequences as visible characters.
PS1=$'\\[\033]133;A\a\\]'"${PS1}"$'\\[\033]133;B\a\\]'
