# bash completion for cairn(1).
#
# The function is deliberately dumb: every word of the line after `cairn` is
# forwarded to `cairn __complete`, which asks the running daemon and answers
# with `value<TAB>description` lines (cairn(1), COMPLETION). Nothing about
# cairn's grammar is known here, so scripts and verbs cannot drift apart.
# An unreachable daemon means no output and a quiet return.

_cairn() {
    local line
    COMPREPLY=()
    while IFS= read -r line; do
        COMPREPLY+=("${line%%$'\t'*}")
    done < <(cairn __complete "${COMP_WORDS[@]:1}" 2>/dev/null)
}
# nosort keeps the daemon's title order instead of an alphabetical re-shuffle.
complete -o nosort -F _cairn cairn
