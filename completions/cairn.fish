# fish completion for cairn(1).
#
# fish understands `value\tdescription` from a custom completion function by
# itself, so this is the thinnest of the three: collect the words before the
# cursor plus the word on it, hand them to `cairn __complete`, print what
# comes back (cairn(1), COMPLETION). No answer from the daemon, no candidates.

function __cairn_complete
    set -l words (commandline -opc)
    if test (count $words) -ge 1
        set -e words[1]
    end
    set -a words (commandline -ct)
    command cairn __complete $words 2>/dev/null
end

complete -c cairn -f -a '(__cairn_complete)'
