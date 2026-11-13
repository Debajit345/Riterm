# riterm shell integration for fish.
#
# Emits iTerm2-compatible OSC 133 prompt marks and OSC 7 working directory.
#
# This file is part of riterm, licensed GPL-3.0-or-later.

if not status is-interactive
    exit
end
if not set -q RITERM
    exit
end
if set -q __RITERM_FISH_LOADED
    exit
end
set -g __RITERM_FISH_LOADED 1

function __riterm_preexec --on-event fish_preexec
    printf '\033]133;C\007'
end

function __riterm_postexec --on-event fish_postexec
    printf '\033]133;D;%s\007' $status
end

# Wrap the user's prompt so we can bracket it with A/B marks.
if functions -q fish_prompt
    functions -c fish_prompt __riterm_original_prompt
else
    function __riterm_original_prompt
        printf '%s> ' (prompt_pwd)
    end
end

function fish_prompt
    printf '\033]7;file://%s%s\007' (hostname) $PWD
    printf '\033]133;A\007'
    __riterm_original_prompt
    printf '\033]133;B\007'
end
