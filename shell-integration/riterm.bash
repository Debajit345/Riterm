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

# PROMPT_COMMAND runs before the prompt is drawn and still sees the exit
# status of the command that just finished. Like iTerm2, emit D on every
# prompt (including the first, where it just closes the previous command).
__riterm_precmd() {
    local status=$?
    __riterm_osc "133;D;${status}"
    __riterm_osc "7;file://${HOSTNAME:-localhost}${PWD}"
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
