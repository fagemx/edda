# Extract machine-readable inline citations; examples in fenced blocks are prose.
# Plain contextual examples are not assertions about current repository files.
BEGIN {
    while ((getline file < tracked_file) > 0)
        if (file ~ /^100(644|755) /) tracked[substr(file, 8)] = 1
    close(tracked_file)
}
function error(message) {
    printf "%s:%d: %s\n", doc, FNR, message > "/dev/stderr"
    failed = 1
}
function citation(token,    path, rest, pos, range, anchor, n, first, last, text, hit, status) {
    path = token
    sub(/[:#].*$/, "", path)
    # File paths, not URLs, Rust symbols, commands, globs or directory names.
    if (path !~ /^[A-Za-z0-9_.\/-]+\.[A-Za-z0-9_-]+$/) return
    rest = substr(token, length(path) + 1)
    range = ""; anchor = ""
    if (rest ~ /^:[0-9]+(-[0-9]+)?(#.*)?$/) {
        range = substr(rest, 2); sub(/#.*/, "", range)
        pos = index(rest, "#"); if (pos) anchor = substr(rest, pos + 1)
    } else if (rest ~ /^#.+$/) {
        # Rust raw member access (value.r#type) is not a filename anchor.
        if (path !~ /\// && path !~ /\.(rs|md|sh|awk|ps1|yml|yaml|toml|json|txt|py|js|ts|lock)$/) return
        anchor = substr(rest, 2)
    } else if (rest != "" || !rust || path !~ /^(crates|tests)\/.*\.rs$/ || example) return
    # Contextual basename shorthand in older prose is not a root citation.
    # The contract page requires full paths; Rust doc comments do too.
    if (path ~ /^edda-[^/]+\//) path = "crates/" path
    if (!strict && anchor == "" && path !~ /^(crates|scripts|docs|tests|\.github)\//) return
    # Markdown under tests/ is test input or recorded review output, not a
    # current source contract. Explicit anchors remain checked everywhere.
    if (doc ~ /^tests\// && anchor == "") return
    if (anchor ~ /[\t\r\n]/) return
    if (strict && range != "" && anchor == "") {
        error("contract line citation requires a literal anchor: " token); return
    }
    if (path ~ /(^|\/)\.\.?(\/|$)/ || path ~ /^\//) {
        error("citation must be repo-root-relative: " token); return
    }
    if (!(path in tracked)) { error("missing citation target " path " (repo root)"); return }
    if (!(path in count)) {
        count[path] = 0
        while ((status = (getline text < ("./" path))) > 0) {
            sub(/\r$/, "", text)
            source[path, ++count[path]] = text
        }
        close("./" path)
        if (status < 0) { error("cannot read citation target " path); return }
    }
    split(range, r, "-"); first=r[1]+0; last=(r[2] == "" ? first : r[2]+0)
    if (range != "" && (first < 1 || last < first || last > count[path])) {
        error("out-of-bounds citation " token); return
    }
    if (anchor != "") {
        for (n=(first ? first : 1); n<=(first ? last : count[path]); n++)
            if (index(source[path, n], anchor)) { hit=1; break }
        if (!hit) error("literal anchor mismatch: " token)
    }
}
FNR == 1 {
    doc = FILENAME; sub(/^\.\//, "", doc)
    rust = (doc ~ /^crates\/.*\.rs$/)
    strict = (doc == "COMPATIBILITY.md")
    fenced = 0
    example = 0
}
{
    line = $0; sub(/\r$/, "", line)
    if (rust) {
        if (line !~ /^[ \t]*\/\/[\/!]/) { example=0; fenced=0; next }
        sub(/^[ \t]*\/\/[\/!] ?/, "", line)
        if (line ~ /^[ \t]*$/) example=0
        if (line ~ /[Ee]\.g\.,|[Ee]xample:/) example=1
    }
    fence_line=line; sub(/^[ \t]*/, "", fence_line)
    if (fence_line ~ /^(```|~~~)/) {
        char=substr(fence_line,1,1); width=0
        while (substr(fence_line,width+1,1) == char) width++
        rest=substr(fence_line,width+1)
        if (!fenced) { fenced=1; fence_char=char; fence_width=width; next }
        if (char == fence_char && width >= fence_width && rest ~ /^[ \t]*$/) { fenced=0; next }
    }
    if (fenced) next
    while (match(line, /`[^`]+`/)) {
        citation(substr(line, RSTART + 1, RLENGTH - 2))
        line = substr(line, RSTART + RLENGTH)
    }
}
END { exit failed }
