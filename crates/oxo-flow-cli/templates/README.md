# Embedded template gallery

Publish-time copies of `examples/gallery/*.oxoflow` for the `template`
command. `cargo package`/`cargo publish` can only bundle files inside the
crate root, so the canonical sources at the workspace root must be mirrored
here.

**Do not edit these files by hand.** The canonical source of truth is
`examples/gallery/`. Sync by copying:

```bash
cp examples/gallery/*.oxoflow crates/oxo-flow-cli/templates/
```

The drift-guard test `embedded_gallery_matches_disk_gallery`
(`src/commands/project.rs`) compares stems and content against
`examples/gallery/` and fails CI on any divergence, so an out-of-sync copy
cannot ship.
