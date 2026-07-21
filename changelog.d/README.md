# Changelog fragments

Every PR that changes user-visible behaviour drops **one new file here** instead of editing
`CHANGELOG.md`. At release time the fragments are assembled into the `[Unreleased]` section and
deleted.

The point is mechanical: `CHANGELOG.md` has a single append point per section, so two branches
open at the same time conflict on it *every time*, and the conflict is always the same
boring "both sides added a bullet" resolution. A file per change has no shared append point,
so parallel branches never touch the same bytes.

## Naming

```
changelog.d/<id>.<category>.md
```

- `<id>` — the PR number if you know it (`47`), otherwise the branch slug (`ocr-geometry`).
  A PR that lands two unrelated changes writes two files.
- `<category>` — one of `added`, `changed`, `deprecated`, `removed`, `fixed`, `security`.
  These are the [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) sections, lowercased.

Examples: `47.added.md`, `ocr-geometry.changed.md`, `52.fixed.md`.

## Contents

The file body is the changelog bullet(s) exactly as they should appear, `-` and all — no heading,
no blank framing lines. Write it for someone upgrading, not for someone reading the diff:
say what changed, and if it breaks something, say what to do about it.

```markdown
- **Batch extraction API.** `POST /api/extract/batch` accepts N documents and returns a job id;
  poll `GET /api/jobs/{id}` for per-document results. Gated on the `batch` license feature.
```

Wrap at 100 columns to match the rest of the repo's prose.

## Assembling a release

```bash
scripts/assemble-changelog.sh          # print the assembled sections to stdout
scripts/assemble-changelog.sh --write  # splice them into CHANGELOG.md and delete the fragments
```

`--write` inserts each fragment under the matching `###` heading of the topmost `##` section of
`CHANGELOG.md`, creating the heading if it does not exist yet. Review the diff before committing —
the script moves text, it does not edit it.
