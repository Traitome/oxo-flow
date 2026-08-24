# Embedded template gallery

Publish-time copies of `examples/gallery/` for the `template` command.
`cargo package`/`cargo publish` can only bundle files inside the crate
root, so the canonical sources at the workspace root must be mirrored
here — both the `*.oxoflow` workflows and the auxiliary files
(`scripts/`, report `templates/`) they reference.

**Do not edit these files by hand.** The canonical source of truth is
`examples/gallery/`. Sync by copying:

```bash
cp examples/gallery/*.oxoflow crates/oxo-flow-cli/templates/
rm -rf crates/oxo-flow-cli/templates/aux
cp -r examples/gallery/scripts examples/gallery/templates crates/oxo-flow-cli/templates/aux/
```

The drift-guard test `embedded_gallery_matches_disk_gallery`
(`src/commands/project.rs`) compares stems and content against
`examples/gallery/` — workflows and aux files alike — and fails CI on
any divergence, so an out-of-sync copy cannot ship. `TEMPLATE_AUX_FILES`
maps template stems to the aux files `template` copies next to a
generated workflow.
