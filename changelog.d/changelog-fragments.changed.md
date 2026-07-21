- **Changelog entries now land as fragments in `changelog.d/` instead of edits to `CHANGELOG.md`.**
  A single append point per section meant every pair of concurrently open branches conflicted on
  the changelog, always with the same "both sides added a bullet" resolution; a file per change
  has no shared append point. `scripts/assemble-changelog.sh --write` splices the fragments into
  the topmost release section at release time and deletes them. Pure bash, no new tooling. See
  [`changelog.d/README.md`](changelog.d/README.md).
