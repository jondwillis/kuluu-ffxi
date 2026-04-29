# Zone floor textures

Drop top-down zone PNGs here, named `{zone_id}.png`. The 3D operator
dashboard (`ffxi-client tui3d ...` with `--features view-3d`) loads
`assets/maps/{zone_id}.png` whenever the player crosses into that zone,
and falls back to a procedural grey grid when no PNG is present.

We ship no images in this repository — distribution of FFXI client-side
art is a license question we don't try to answer. Common community sources
people use for these PNGs:

- [Windower's `maps` resource](https://github.com/Windower/Resources/) —
  top-down PNG sets per zone.
- BG-Wiki SVG / image exports.

## `zone_meta.json` (optional)

For zones whose PNG isn't centered on the world origin, or that cover
more or less than the default 200×200 world units, override per-zone:

```json
{
  "115": {
    "image": "west_sarutabaruta.png",
    "world_origin": [0.0, 0.0],
    "scale": [1024.0, 1024.0]
  },
  "234": {
    "world_origin": [-100.0, 50.0],
    "scale": [400.0, 400.0]
  }
}
```

- `image` (optional): filename relative to this dir; defaults to `{zone_id}.png`.
- `world_origin`: FFXI world coords (x, y) of the texture's center pixel.
- `scale`: world units covered by the texture's [width, height].

## Real terrain (heightmap)

Zone elevation lives in client `.DAT` files. If you set
`FFXI_DAT_PATH=...` we log that we noticed but still use the PNG floor —
parsing the DAT-side terrain mesh is a separate (much larger) project.
