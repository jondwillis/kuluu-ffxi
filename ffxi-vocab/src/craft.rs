//! Synthesis law as data: the recipe table LSB loads at boot and the crystal
//! element tiers its `crystalProps` collapses. Built by `scrape_synth` in
//! ffxi-vocab/build.rs from vendor/server/sql/synth_recipes.sql and
//! vendor/server/src/map/utils/synthutils.cpp; the server keys incoming synth
//! asks against exactly these rows (`resolveRecipe`, `SynthRecipe::ingredientKey`).

/// One element's three crystal tiers, in the order `crystalProps` lists them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CrystalGroup {
    /// The `ELEMENT_` name from vendor/server/src/map/items.h (e.g. "FIRE").
    pub element: &'static str,
    pub crystals: [u16; 3],
}

/// One active row of synth_recipes.sql, fields in the SQL's column order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SynthRecipe {
    pub id: u32,
    /// `Desynth` — CRAFT_DESYNTHESIS (vendor/server/src/map/items/craft_state.h).
    pub desynth: bool,
    /// Required key item id (`KeyItem`), 0 when none.
    pub key_item: u32,
    /// Difficulty per craft skill, Woodworking..Cooking order; the server
    /// refuses when `skill < required * 10 - 15`.
    pub skill_required: [u8; 8],
    /// The recipe's listed crystal and its HQ upgrade (`Crystal`, `HQCrystal`).
    pub crystal: u16,
    pub hq_crystal: u16,
    /// Ingredient item ids in the row's own order — `ingredientKey` joins them
    /// positionally, so a synth ask must offer them in exactly this order.
    /// Trailing zeros mean empty ingredient slots.
    pub ingredients: [u16; 8],
    pub result: u16,
    /// `ResultHQ1..3` — LSB rows repeat the normal result here today.
    pub result_hq: [u16; 3],
    /// Quantities for the normal result, then HQ1..3 (`ResultQty`, `ResultHQ*Qty`).
    pub qty: [u8; 4],
    /// `ResultName`, verbatim SQL text (also what retail's craft list shows).
    pub name: &'static str,
    /// Expansion gate name from `content_tag` (`NULL` → None); the server drops
    /// recipes whose tag is disabled (`resolveRecipe`'s `IsContentEnabled`).
    pub content_tag: Option<&'static str>,
}

include!(concat!(env!("OUT_DIR"), "/craft_table.rs"));

pub fn recipe(id: u32) -> Option<&'static SynthRecipe> {
    SYNTH_RECIPES.iter().find(|r| r.id == id)
}

/// The tier group a crystal belongs to (`crystalProps` case chains).
pub fn crystal_group(crystal: u16) -> Option<&'static CrystalGroup> {
    SYNTH_CRYSTAL_GROUPS
        .iter()
        .find(|g| g.crystals.contains(&crystal))
}

/// Whether `held` can stand in for a recipe's `required` crystal: the server's
/// recipe key runs every offered crystal through `crystalString`, which folds
/// all three tiers of one element into a single name (both in
/// vendor/server/src/map/utils/synthutils.cpp).
pub fn crystal_accepts(required: u16, held: u16) -> bool {
    required == held || crystal_group(required).is_some_and(|g| g.crystals.contains(&held))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recipe_table_matches_the_vendor_row_count() {
        assert!(
            SYNTH_RECIPES.len() >= 4000,
            "synth_recipes scrape regressed to {} rows",
            SYNTH_RECIPES.len()
        );
        assert!(SYNTH_RECIPES.windows(2).all(|w| w[0].id < w[1].id));
    }

    #[test]
    fn spot_check_rows_match_the_sql_text() {
        let table = recipe(1).expect("row 1");
        assert_eq!(table.name, "Banquet Table");
        assert_eq!((table.crystal, table.hq_crystal), (4099, 4241));
        assert_eq!(table.ingredients, [3553, 3554, 3555, 0, 0, 0, 0, 0]);
        assert_eq!(table.content_tag, Some("ABYSSEA"));

        let lumber = recipe(2).expect("row 2");
        assert_eq!(lumber.name, "Arrowwood Lumber");
        assert_eq!(lumber.qty, [1, 2, 3, 4]);
        assert_eq!(lumber.content_tag, None);
        assert!(recipe(16).expect("row 16").desynth);
    }

    #[test]
    fn every_recipe_crystal_lands_in_one_element_group() {
        for r in SYNTH_RECIPES {
            assert!(crystal_group(r.crystal).is_some(), "recipe {}", r.id);
            assert!(crystal_group(r.hq_crystal).is_some(), "recipe {}", r.id);
        }
    }

    /// The SQL trigger `ensure_synth_ingredients_are_ordered` keeps the eight
    /// Ingredient columns zero-suffixed, so dropping zeros preserves positions —
    /// what a synth ask sends as its positional Items list.
    #[test]
    fn ingredient_columns_are_zero_suffixed() {
        for r in SYNTH_RECIPES {
            let nonzero = r.ingredients.iter().filter(|&&i| i != 0).count();
            assert!(
                r.ingredients[..nonzero].iter().all(|&i| i != 0),
                "recipe {}",
                r.id
            );
        }
    }

    #[test]
    fn crystal_substitution_follows_the_tier_groups() {
        let fire = crystal_group(4096).expect("FIRE_CRYSTAL");
        assert_eq!(&fire.crystals, &[4096, 4238, 6506]);
        // INFERNO_CRYSTAL is the same element tier (items.h / crystalProps law).
        assert!(crystal_accepts(4096, 4238));
        assert!(!crystal_accepts(4096, 4102));
    }
}
