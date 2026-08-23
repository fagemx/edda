#!/bin/sh
set -eu

root=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' 0 HUP INT TERM
case_number=0

check() {
    expected=$1
    name=$2
    file=$3
    content=$4
    case_number=$((case_number + 1))
    case_dir="$tmp/$case_number"
    mkdir -p "$case_dir/scripts"
    cp "$root/scripts/lint-markdown-content.sh" "$case_dir/scripts/"
    printf '%s\n' "$content" >"$case_dir/$file"
    git -C "$case_dir" init -q
    git -C "$case_dir" config core.autocrlf false
    git -C "$case_dir" add -- "$file"

    if output=$(cd "$case_dir" && sh scripts/lint-markdown-content.sh 2>&1); then
        actual=pass
    else
        actual=fail
    fi
    if [ "$actual" != "$expected" ]; then
        printf '%s: expected %s, got %s\n%s\n' "$name" "$expected" "$actual" "$output" >&2
        exit 1
    fi
}

check pass 'fenced transcripts and spaced filename' 'spaced name.md' '
````text
running 1 test
test result: ok. 1 passed
test crate::case ... ok
 Compiling demo v0.1.0
````

~~~~
Running tests
~~~~
'

check pass 'list-nested fenced transcript' 'nested.md' '
- build transcript

    ```text
    Compiling demo v0.1.0
    ```
'

check fail 'running signature outside a fence' 'running.md' 'running 1 test'
check fail 'result signature outside a fence' 'result.md' 'test result: FAILED. 0 passed'
check fail 'test signature outside a fence' 'test.md' 'test crate::case ... ignored'
check fail 'cargo signature outside a fence' 'cargo.md' 'Checking demo v0.1.0'
check fail 'backtick in info string is not a fence' 'info.md' '
```bad`info
Compiling demo v0.1.0
```
'
check fail 'list-like fence content does not hide later output' 'fence-list.md' '
- transcript

    ```text
    - literal output
    ```

Checking demo v0.1.0
'

printf 'lint-markdown-content fixtures passed\n'
