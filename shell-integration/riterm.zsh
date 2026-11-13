# riterm shell integration for zsh.
#
# Emits iTerm2-compatible OSC 133 prompt marks and OSC 7 working directory.
#
# This file is part of riterm, licensed GPL-3.0-or-later.

[[ -o interactive ]] || return 0
[[ -n "${RITERM:-}" ]] || return 0
[[ "${__RITERM_ZSH_LOADED:-}" == 1 ]] && return 0
__RITERM_ZSH_LOADED=1

autoload -Uz add-zsh-hook

__riterm_preexec() {
    __riterm_command_started=1
    printf '\033]133;C\007'
}

__riterm_precmd() {
    local status=$?
    if [[ "${__riterm_command_started:-0}" == 1 ]]; then
        printf '\033]133;D;%s\007' "$status"
        __riterm_command_started=0
    fi
    printf '\033]7;file://%s%s\007' "${HOST:-localhost}" "$PWD"
}

add-zsh-hook preexec __riterm_preexec
add-zsh-hook precmd __riterm_precmd

# Bracket the prompt: A at its start, B at its end.
PROMPT=$'%{\033]133;A\a%}'"${PROMPT}"$'%{\033]133;B\a%}'
