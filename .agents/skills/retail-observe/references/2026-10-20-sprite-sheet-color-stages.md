# Sprite-sheet color and alpha stages

## Scope

This independent reader investigation covers ordinary sprite-sheet resources in `retail-2026-09`.
It establishes vertex color upload and the sheet-specific texture-stage predicates.
It does not establish a complete lamp intensity, size, or lifetime specification.
No implementation change or original-client input occurred.
The reader stops after this record.
A separate implementation session must use only the specification sections, with disassembly closed.

## Vertex color

The original client uploads each authored sprite-sheet vertex color as packed bytes without a color-scale conversion.
The graphics pipeline interprets each diffuse component as its byte value divided by 255.
Thus authored byte 128 enters the first texture stage as `128/255`, not 1.

A software renderer may combine this normalization with an explicit factor of two.
That algebra does not permit another factor of two at the same stage.
It must also preserve saturation between successive stages.
Using `byte/128` is an approximation to `2*byte/255`, not an exact normalization rule.

## Normal textured sheet

Let `D` be the normalized interpolated vertex color.
Let `T` be the sampled texture color.
Let `F` be the normalized particle factor color.
Let `sat(x)` clamp each component to the range zero through one.

The normal sprite-sheet output is:

```text
rgb0 = sat(2 * D.rgb * T.rgb)
rgb1 = sat(2 * rgb0 * F.rgb)
a0   = sat(2 * D.a * T.a)
a1   = sat(4 * a0 * F.a)
```

The two RGB operations and the final alpha operation match the corresponding normal textured D3m operations.
This similarity does not remove the sprite-sheet branch predicates below.
The factor color remains a separate input; it does not replace the authored vertex color.

## Sheet predicates

If bit 28 of the authored setup flags is set, the first alpha stage selects `D.a` directly.
It does not read the texture alpha or multiply that first alpha by two.
The final alpha stage still computes `sat(4 * D.a * F.a)`.
The RGB stages remain unchanged.

If the low byte of the authored render-mode value is `0x40`, the final alpha stage selects `F.a` directly.
This override applies after either first-stage alpha selection.
It therefore removes texture alpha and vertex alpha from the final source alpha.
It does not change the two RGB stages.

The render-mode value comes from generator initializer opcode `0x1E`.
The setup flag comes from the standard setup flags word.
These are distinct authored inputs.
A renderer must not substitute a D3m-only predicate without verifying the same sheet condition.

## Observable consequences and limits

Vertex byte 128 alone does not imply half-opacity output after the normal first stage.
The first stage also doubles the product with texture alpha.
The second stage scales that result by the factor alpha and four.
Either stage can saturate.

The final source alpha therefore cannot be inferred from the factor alpha alone in the normal textured case.
Conversely, render mode `0x40` makes the factor alpha decisive regardless of texture alpha.
These rules can change the visible size of a soft halo because the texture's outer alpha gradient controls coverage.
They do not justify an arbitrary size, brightness, alpha, or flicker gain.

The actual Woods generator flags must select the applicable branch before computing a lamp result.
This investigation did not independently audit those generator values or trace every blend-factor selection.
The installed HXI build was not inspected for these stage predicates.
The earlier HXI/Wine lamp captures remain separate visible evidence.

## Provenance

Build: `retail-2026-09`, patch `30260904_1`.
DLL SHA-256: `f2245d1c9d06e02c36624942483913f5120c0d40777fc1bb8703c6f4bda823e4`.
Unpacked text SHA-256: `b55f8b4c730c00229e2febd1ea6a5efba29920e094fa565a3816b763c3d9cdc9`.
Image base VA: `0x10000000`; text begins at RVA `0x1000`.
The registered retail install supplied the DLL.
The existing hash-matched dump was read with Capstone.

Community locator only: `research/XIClient/src/XIClient/source/Resource/Derived/CMoD3a.cpp`, `CMoD3a::Open`.
Its element counterpart is `World/Generator/Effects/CMoD3aElem.cpp`, `PrepD3aTSS` and `OnDraw`.
The main checkout's populated XIClient reference supplied these locator hypotheses.
The production findings below were checked independently in the build-matched DLL.

Resource upload RVA `0x3e6c0` initializes vertex format `0x152` at RVA `0x3e7eb` through `0x3e7f8`.
The loop at RVAs `0x3e83c` through `0x3e883` copies the packed DAT color into the diffuse member unchanged.
The vertex stride is `0x24`; the packed diffuse member is at offset `0x18`.
Only position, normal, and UV members receive the other writes in this loop.

Resource drawing at RVA `0x3e910` selects the frame and binds the texture.
It invokes sheet stage setup at RVA `0x3e955`, then submits through RVA `0x43f40`.
Sheet setup RVA `0x3e9a0` tests element setup bit 28 and selects state table VA `0x10351ec8` or `0x10351fa0`.
Both tables use COLOROP 5 at stages zero and one.
The normal table uses ALPHAOP 5 at stage zero and 6 at stage one.
The bit-28 table uses ALPHAOP 2 with CURRENT at stage zero, then ALPHAOP 6 at stage one.
The stage-zero CURRENT argument is diffuse for this fixed-function input.
The method disables lighting and applies the factor through render state `0x3c`.

At RVA `0x3eaad`, the low render-mode byte is compared with `0x40`.
The matching branch applies table VA `0x10352084`.
That table sets stage-one ALPHAOP 3 with argument two equal to TFACTOR.

Setup transfer at RVA `0x513a3` through `0x513ad` copies the standard setup payload flags unchanged into element offset `0x10c`.
Initializer opcode `0x1E` dispatches through table RVA `0x5247c` to RVA `0x51d05`.
It copies the authored 16-bit value into element offset `0x16c`.

Direct3D operations are defined by Microsoft's `D3DTEXTUREOP` documentation:
<https://learn.microsoft.com/en-us/windows/win32/direct3d9/d3dtextureop>.
Packed diffuse uses the standard Direct3D color input, not a client-specific floating-point normalization.
Saved disassembly and extracted state rows are under `artifacts/verify/release-080-current-main/retail-reference/lamp-stage-reader/`.
