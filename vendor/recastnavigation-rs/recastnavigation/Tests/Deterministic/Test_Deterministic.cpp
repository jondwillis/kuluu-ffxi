#include "Deterministic.h"

#include "catch2/catch_all.hpp"

#include "math.h"

TEST_CASE("dmSinCos", "[deterministic]")
{
	SECTION("edge case")
	{
		auto res = dmSinCos(0);
		REQUIRE(fabsf(res.sin - 0) < 1.0e-7f);
		REQUIRE(fabsf(res.cos - 1) < 1.0e-7f);

		res = dmSinCos(FRAC_PI_2);
		REQUIRE(fabsf(res.sin - 1) < 1.0e-7f);
		REQUIRE(fabsf(res.cos - 0) < 1.0e-7f);

		res = dmSinCos(PI);
		REQUIRE(fabsf(res.sin - 0) < 1.0e-7f);
		REQUIRE(fabsf(res.cos - -1) < 1.0e-7f);

		res = dmSinCos(-FRAC_PI_2);
		REQUIRE(fabsf(res.sin - -1) < 1.0e-7f);
		REQUIRE(fabsf(res.cos - 0) < 1.0e-7f);
	}

	SECTION("general")
	{
		double ms = 0.0, mc = 0.0;
		
		for (float x = -100.0f * PI; x < 100.0f * PI; x += 2.5e-4f) {
			auto res = dmSinCos(x);

			double s1 = sin(double(x));
			double ds = abs(res.sin - s1);
			ms = ms > ds ? ms : ds;

			double c1 = cos(double(x));
			double dc = abs(res.cos - c1);
			mc = mc > dc ? mc : dc;
		}
		
		REQUIRE(ms < 1.0e-7);
		REQUIRE(mc < 1.0e-7);
	}
}

TEST_CASE("dmASin", "[deterministic]")
{
	SECTION("edge case")
	{
		REQUIRE(dmASin(0.0) == 0.0);
		REQUIRE(dmASin(1.0) == FRAC_PI_2);
		REQUIRE(dmASin(-1.0) == -FRAC_PI_2);

		REQUIRE(dmASin(1.1) == FRAC_PI_2);
		REQUIRE(dmASin(-1.1) == -FRAC_PI_2);
	}

	SECTION("general")
	{
		double ma = 0.0;

		for (float x = -1.0f; x <= 1.0f; x += 2.5e-4f) {
			double a1 = asin((double)x);
			double da = abs(dmASin(x) - a1);
			ma = ma > da ? ma : da;
		}

		REQUIRE(ma < 2.0e-7);
	}
}

TEST_CASE("dmACos", "[deterministic]")
{
	SECTION("edge case")
	{
		REQUIRE(dmACos(0.0) == FRAC_PI_2);
		REQUIRE(dmACos(1.0) == 0.0);
		REQUIRE(dmACos(-1.0) == PI);

		REQUIRE(dmACos(1.1) == 0.0);
		REQUIRE(dmACos(-1.1) == PI);
	}

	SECTION("general")
	{
		double ma = 0.0;

		for (float x = -1.0f; x <= 1.0f; x += 2.5e-4f) {
			double a1 = acos((double)x);
			double da = abs(dmACos(x) - a1);
			ma = ma > da ? ma : da;
		}

		REQUIRE(ma < 3.5e-7);
	}
}
