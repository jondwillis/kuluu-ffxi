[![Release Doc](https://docs.rs/recastnavigation-rs/badge.svg)](https://docs.rs/recastnavigation-rs)
[![Crate](https://img.shields.io/crates/v/recastnavigation-rs.svg)](https://crates.io/crates/recastnavigation-rs)
![github actions](https://github.com/SlimeYummy/recastnavigation-rs/actions/workflows/main.yml/badge.svg)
[![CircleCI](https://dl.circleci.com/status-badge/img/gh/SlimeYummy/recastnavigation-rs/tree/master.svg?style=shield)](https://dl.circleci.com/status-badge/redirect/gh/SlimeYummy/recastnavigation-rs/tree/master)

# Recastnavigation-rs

Recastnavigation-rs is a rust wrapper for [recastnavigation](https://github.com/recastnavigation/recastnavigation) pathfinding library with cross-platform deterministic.

To import deterministic support, this project use a special fork of recastnavigation [recastnavigation-deterministic](https://github.com/SlimeYummy/recastnavigation-deterministic). So it can be used in network game scenarios, such as lock-step networking synchronize.

### Features

We plan to support all features in original recastnavigation C++ project. Currently, recast/detour/detour_crowd are implemented. If the feature you need is not implemented, you can create an issue.

### Examples

The test cases under [./tests](https://github.com/FenQiDian/recastnavigation-rs/tree/master/tests) can be viewed as examples.

Recastnavigation-rs keeps the same API styles with original recastnavigation library. Therefore, you can also refer to the recastnavigation [demo](https://github.com/recastnavigation/recastnavigation/tree/main/RecastDemo).

### Platforms

In theory, recastnavigation-rs supports all platforms supported by rust. But I only tested on the following platforms:
- Windows/Ubuntu/Mac x64 (Github actions)
- X64/Arm64 docker ([CircleCI](https://dl.circleci.com/status-badge/redirect/gh/SlimeYummy/recastnavigation-rs/tree/master))

Maybe you can run cross-platform deterministic test cases under [./tests](https://github.com/FenQiDian/recastnavigation-rs/tree/master/tests) on your target platform.
