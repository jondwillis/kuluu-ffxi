#pragma once

// Local patch (ffxi workspace): the upstream header references `size_t`
// at line 66 (`dmQsort(void*, size_t, size_t, DmCmp)`) without including
// anything that guarantees the typedef. Direct `.cpp` includers (Qsort.cpp,
// Math.cpp) bring it in via `<stdlib.h>`, but cxx-bridge-generated TUs
// don't, which surfaces under `cargo test` after the bridge pulls in
// every demo/include. Upstream this would be a one-line PR to
// SlimeYummy/recastnavigation-rs.
#include <cstddef>

#include "math.h"

//
// math functions
//

inline bool dmIsFinite(float n) {
	return isfinite(n);
}

inline bool dmIsNan(float n) {
	return isnan(n);
}

inline float dmAbs(float n) {
	return fabsf(n);
}

inline float dmSqrt(float n) {
	return sqrtf(n);
}

inline float dmFloor(float n) {
	return floorf(n);
}

inline float dmCeil(float n) {
	return ceilf(n);
}

const float PI = 3.14159265358979323846264338327950288;
const float FRAC_PI_2 = 1.57079632679489661923132169163975144;
const float FRAC_2_PI = 0.636619772367581343075535053490057448;

struct DmSinCos {
    float sin;
    float cos;
};

DmSinCos dmSinCos(float n);

inline float dmSin(float n) {
	DmSinCos res = dmSinCos(n);
	return res.sin;
}

inline float dmCos(float n) {
	DmSinCos res = dmSinCos(n);
	return res.cos;
}

float dmASin(float n);

inline float dmACos(float n) {
	return FRAC_PI_2 - dmASin(n);
}

//
// sqort
//

typedef int (*DmCmp)(const void*, const void*);

void dmQsort(void* base, size_t nel, size_t width, DmCmp cmp);
