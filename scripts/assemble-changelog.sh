#!/usr/bin/env bash
# Assemble changelog.d/ fragments into CHANGELOG.md's topmost release section.
#
#   scripts/assemble-changelog.sh           print the assembled sections to stdout
#   scripts/assemble-changelog.sh --write   splice them in and delete the fragments
#
# See changelog.d/README.md for the fragment naming convention. Pure bash — no towncrier,
# no Python, nothing to install.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
frag_dir="$repo_root/changelog.d"
changelog="$repo_root/CHANGELOG.md"

# Keep a Changelog's canonical section order.
categories=(added changed deprecated removed fixed security)

write=0
case "${1:-}" in
  --write) write=1 ;;
  "")      ;;
  *)       echo "usage: $(basename "$0") [--write]" >&2; exit 2 ;;
esac

title_of() {
  case "$1" in
    added) echo Added ;; changed) echo Changed ;; deprecated) echo Deprecated ;;
    removed) echo Removed ;; fixed) echo Fixed ;; security) echo Security ;;
  esac
}

shopt -s nullglob

# `$'\r'` is not ANSI-C-expanded inside double quotes, so the strip pattern needs a variable.
cr=$'\r'

# Read a fragment: strip CR, strip trailing blank lines, into the global `body` array.
read_fragment() {
  body=()
  local line
  while IFS= read -r line || [ -n "$line" ]; do body+=( "${line%"$cr"}" ); done < "$1"
  while [ ${#body[@]} -gt 0 ] && [ -z "${body[-1]}" ]; do unset 'body[-1]'; done
}

# ---------------------------------------------------------------- print mode
if [ "$write" -eq 0 ]; then
  found=0
  for cat in "${categories[@]}"; do
    files=( "$frag_dir"/*."$cat".md )
    [ ${#files[@]} -eq 0 ] && continue
    found=1
    printf '### %s\n' "$(title_of "$cat")"
    for f in "${files[@]}"; do read_fragment "$f"; printf '%s\n' "${body[@]}"; done
    printf '\n'
  done
  [ "$found" -eq 0 ] && echo "no fragments in changelog.d/" >&2
  exit 0
fi

# ---------------------------------------------------------------- write mode
pending=()
for cat in "${categories[@]}"; do
  files=( "$frag_dir"/*."$cat".md )
  [ ${#files[@]} -gt 0 ] && pending+=( "${files[@]}" )
done
if [ ${#pending[@]} -eq 0 ]; then
  echo "no fragments in changelog.d/ — nothing to do" >&2
  exit 0
fi

# Preserve the file's existing line terminator.
eol=$'\n'
grep -qU $'\r' "$changelog" && eol=$'\r\n'

lines=()
while IFS= read -r line || [ -n "$line" ]; do lines+=( "${line%"$cr"}" ); done < "$changelog"

# The topmost `## ` heading is the release being assembled; the next one ends it.
start=-1
for ((i = 0; i < ${#lines[@]}; i++)); do
  case "${lines[i]}" in "## "*) start=$i; break ;; esac
done
[ "$start" -lt 0 ] && { echo "CHANGELOG.md has no '## ' section" >&2; exit 1; }
end=${#lines[@]}
for ((i = start + 1; i < ${#lines[@]}; i++)); do
  case "${lines[i]}" in "## "*) end=$i; break ;; esac
done

sec=( "${lines[@]:start:end-start}" )

# Append `body` under `### $1`, at the end of that subsection, creating the heading if absent.
insert_into_section() {
  local hdr="### $1" idx=-1 ins i
  for ((i = 0; i < ${#sec[@]}; i++)); do [ "${sec[i]}" = "$hdr" ] && { idx=$i; break; }; done
  if [ "$idx" -ge 0 ]; then
    ins=${#sec[@]}
    for ((i = idx + 1; i < ${#sec[@]}; i++)); do
      case "${sec[i]}" in "### "*) ins=$i; break ;; esac
    done
    while [ "$ins" -gt $((idx + 1)) ] && [ -z "${sec[ins-1]}" ]; do ins=$((ins - 1)); done
    sec=( "${sec[@]:0:ins}" "${body[@]}" "${sec[@]:ins}" )
  else
    ins=${#sec[@]}
    while [ "$ins" -gt 0 ] && [ -z "${sec[ins-1]}" ]; do ins=$((ins - 1)); done
    sec=( "${sec[@]:0:ins}" "" "$hdr" "${body[@]}" "${sec[@]:ins}" )
  fi
}

for cat in "${categories[@]}"; do
  files=( "$frag_dir"/*."$cat".md )
  [ ${#files[@]} -eq 0 ] && continue
  merged=()
  for f in "${files[@]}"; do read_fragment "$f"; merged+=( "${body[@]}" ); done
  body=( "${merged[@]}" )
  insert_into_section "$(title_of "$cat")"
done

tmp="$changelog.tmp$$"
: > "$tmp"
for line in "${lines[@]:0:start}" "${sec[@]}" "${lines[@]:end}"; do
  printf '%s%s' "$line" "$eol" >> "$tmp"
done
mv "$tmp" "$changelog"

rm -f -- "${pending[@]}"
printf 'assembled %d fragment(s) into %s\n' "${#pending[@]}" "$(basename "$changelog")" >&2
printf 'review the diff before committing.\n' >&2
