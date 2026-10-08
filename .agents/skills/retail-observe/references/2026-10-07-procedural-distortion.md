# Procedural distortion generator

Observed 2026-10-07. Scope: clean retail-2026-09, patch 30260904_1. This is an independently read interoperability specification from the installed client and its authored DAT inputs. No retail process or Kuluu rendering was run for this observation. It establishes the computation below, not a screenshot comparison or other client-era guarantee.

## Inputs and resource resolution

For an ordinary generator whose standard setup selects element type 0x22, the element is procedural. Its linked four-character name does not require a named mesh, image or sprite sheet. The element can be created when that named resource is absent. This is a type-specific rule, not a missing-resource fallback for other element types.

The clean global generator g142 selects type 0x22 and the name `dist`. Its setup provides scale [1,1,1], a 45-frame lifetime, target-actor attachment, a 0.02 haze parameter, and an alpha track named k143. Independently walking DAT 0 and comparing the production asset inventory found no mesh/image/sprite-sheet named `dist` in the global DAT 0/216 set. The retail resolver deliberately bypasses named-resource lookup for this element type.

## Footprint and scene sampling

The element's built-in local footprint is the center plus four axis endpoints: [0,0,0], [-1,0,0], [0,-1,0], [1,0,0], [0,1,0]. Apply the element's ordinary authored scale/placement and draw transform to this footprint. There is no independent fixed screen-pixel radius in this path.

The five transformed points are projected to screen coordinates. Their bounding rectangle is copied from the already rendered scene into an intermediate texture. Source texture coordinates are the original projected screen coordinates divided by viewport width/height. Capture dimensions at or above 256 are reduced using 255 divided by the projected extent on that axis; smaller extents retain scale one. This capture limit is not a world-space radius or an effect-strength constant.

The final coverage is a fan joining center, the four ordered rim points, and the first rim point again. It is a diamond in the untransformed local plane, with four triangles. The texture is the captured scene, not an authored gradient image. Its coordinates come from the unshifted projected footprint. The haze parameter changes the final draw translation along both the first and second draw axes, while those sampling coordinates remain unchanged. Thus the image displaced into the fan is scene content from the original footprint. Projection and attachment determine the resulting screen displacement; 0.02 is not a 0.02-pixel or viewport-UV offset.

## Coverage, color and time

The center receives the current element color/alpha; the four rim vertices and repeated closing vertex have RGB [128,128,128] and alpha zero. Raster interpolation supplies the spatial coverage gradient from center to transparent rim. This is not a gradient texture and does not establish a radial circular falloff.

The copy pass sets the texture factor to RGBA [128,128,128,128]. The final fan uses doubled sampled RGB times interpolated vertex RGB. Its source alpha is four times interpolated vertex alpha times the texture-factor alpha, clamped by the graphics pipeline. It blends into the scene with source-alpha / one-minus-source-alpha factors. For normalized vertex alpha a this yields nominal coverage clamp(4 * a * 128/255, 0, 1), before rasterization/quantization details. These operations use standard [Direct3D texture operations](https://learn.microsoft.com/en-us/windows/win32/direct3d9/d3dtextureop) and [blend factors](https://learn.microsoft.com/en-us/windows/win32/direct3d9/d3dblend).

For g142, its tick script first computes progress as one minus remaining life divided by initial life, then updates position, then samples the bound alpha track. The track binding selects linear interpolation, not spline interpolation. Sampled alpha is multiplied by 255, negative values become zero, and conversion retains an 8-bit alpha. The draw's current element alpha can additionally be scaled by ordinary element visibility multipliers. The authored k143 knots are:

| Progress | Value |
| --- | --- |
| 0 | 0 |
| 0.26666688919067383 | 0.4799996614456177 |
| 0.7260417938232422 | 0.47999972105026245 |
| 1 | 0.010000495240092278 |

The envelope changes coverage alpha. In this g142 tick script it does not change the initialized 0.02 haze translation. Thus the displacement remains fixed while its contribution fades in/out. Multiplying both displacement and alpha by k143 is not the behavior of this authored path. Exact first/last presented frame, screen capture ordering relative to other effects, and live occlusion/visibility multipliers were not observed.

## Confidence and limits

High confidence: procedural resource eligibility; five-point footprint; scene-copy sampling; fan coverage gradient; separate haze/alpha channels; g142 authored values and linear k143 sampling. These are independently traced in the exact clean retail binary and checked against the clean installed DAT bytes.

Not established: live retail pixels, reverse depth/near-plane clipping, full attachment-joint/camera matrix policies, multisample/stereo differences, effect ordering among concurrent generators, or parity across horizonxi-2023/other builds. A production runtime capture is still required for an implementation. This record does not authorize a guessed radius, custom noise/gradient texture, simplified attachment policy, or reuse of a generic missing-resource fallback.

## Provenance

Image base VA 0x10000000. All following code locations are RVAs in retail-2026-09. DLL SHA256 f2245d1c9d06e02c36624942483913f5120c0d40777fc1bb8703c6f4bda823e4; verified installed file `/Users/jon/Library/Application Support/kuluu/installs/retail/SquareEnix/FINAL FANTASY XI/FFXiMain.dll`. The existing unpacked raw .text `/private/tmp/emission-retail.text.bin` was independently hashed: b55f8b4c730c00229e2febd1ea6a5efba29920e094fa565a3816b763c3d9cdc9, matching the trusted generator-emission record. DLL was not executed or replaced. Capstone 5.0.7 and cached pefile imported read-only; no dependency install.

Resource resolver RVA 0x49570 reads the setup type, explicitly branches on 0x22 at 0x495ca to 0x4968e, records sentinel1 and returns without the named-resource search at 0x495f8. Initializer dispatch RVA0x4e8d0/table0x5247c maps opcode1 to0x50d26. Type0x22 maps via0x52754/0x5271c to0x5102e; ordinary allocation at0x5105f calls constructor0x52d40 and installs vtableVA0x1032b298. Dist draw entry at vtable+0x48 is RVA0x426f0; no named-resource texture read occurs in this draw.

Draw RVA0x42719..0x427ab initializes the five vectors;0x42816 projects all5;0x4282c..0x4288d derives sceneUV;0x42896 obtains the existing scene texture.0x428a4..0x42932 obtains its bounds;0x42936..0x429a4 limits dimensions.0x429a6..0x42a54 constructs copy strip;0x42a59..0x42af7 switches target/binds scene texture/draws.0x42b1f..0x42be1 constructs6vertex fan;centercolor fromVA0x1047bc0c, rimcolors0x00808080.0x42c17..0x42c2c adds scalar element+0x194 to matrixtranslation+0x30/+0x34, leaving capturedUVs unchanged;0x42c40..0x42ca7 binds capturedtexture/fanstate/draws.

Render-state tables RVA0x3530d0 (copy: factor0x80808080),0x353178 (fan: alpha blending1, SRCBLEND5, DESTBLEND6), texture-stage table0x353108 (stage0 COLOROP5, COLORARG1=2/TEXTURE, COLORARG2=1/CURRENT; ALPHAOP6, ALPHAARG1=0/DIFFUSE, ALPHAARG2=3/TFACTOR). Stateblock helperRVA0x6ae30 independently confirmed pair/triple traversal and real device calls. Numeric constantRVA0x329e94=1/255,0x329a20=255,0x32a344=256.

Initializer opcode0x32 handlerRVA0x51b4b takes payload secondfloat;vtable+0x34/RVA0x52d70 stores haze at element+0x194. Opcode0x2d handlerRVA0x50c82..0x50ccd binds track/flags/initialalpha;helperRVA0x4e290 addresses color alpha byte. TickRVA0x4a9e1/table0x4e03c maps0x0e to0x4d6bd: helpers0x4e440/0x4e450 give initialLife/remainingLifeRatio;0x1b to0x4bd50: track reader0x4e4e0 tests low flag nibble and calls linear0x544d0/0x54500 when zero; resulting alpha is scaled255, lower-clamped and8bitretained. General draw prep0x46070 multiplies element+0x134 and+0x138 into alphahelper0x3e330. Haze is not modified by any g142 tick opcode.

Raw DAT0 `/Users/jon/Library/Application Support/kuluu/installs/retail/SquareEnix/FINAL FANTASY XI/ROM/0/0.DAT`:539776bytes, SHA2565b2ac1bc3efbb73a3c9ffbb884ed6b7066355910a91635bfeef14bd027c6c158. DAT216 `ROM/1/0.DAT`:10812144bytes,SHA2564553141d2e4a1f7c6a4b8061945c52d421c029fd3ec73a673509e8c2c6e06f6d. Independently walked DAT chunk format, saved exactg142/k143 extraction in `g142-authored-input.json`. g142 chunk fileoffset363072,384bytes,kind5,bodySHA2569627743076b629fe28e742ab95b84e4b17ac8aff1acbc9e0a2cdf30e4971ca32. k143 fileoffset365552,48bytes,kind25,bodySHA2564f305e474a36c8094419e265b4df12369a346dc1663d36ee5530de5250441fed. Setup sections0x74/0x78 hold init/tick. g142init0x01 selects0x22/life45,0x0f scale1,0x2d binds k143 with flags0,0x32 secondfloat0.02. Tick opcodes0x0e,0x02,0x1b. Production asset inventory corroboration: `/private/tmp/kuluu-pr992-main-proof/critical/frames/chain-assets.json`, only global_files g142/distortion rows read.

Community XIClient CMoDistElem/CYyGenerator and existing Kuluu parsing code were used only as locator/format hypotheses. Every behavior stated above was checked in installed retail code and authored bytes; no community implementation or internal layout is an implementation source. Own disassemblies/helpers remain in this temporary observation workspace and must not be read by the implementation writer.
