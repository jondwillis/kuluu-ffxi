//! The merit point and job point tables behind the `meritcat`, `merit2ca` and `jbpcat` windows
//! (.agents/skills/retail-observe/references/2026-10-15-merit-and-job-point-windows.md). Every id
//! resolves through the install's FTABLE like any other file id.

/// Merit category records: the whole file read as [`CATEGORY_RECORDS`] records of
/// [`CATEGORY_RECORD_LEN`] bytes.
pub const MERIT_CATEGORY_FILE_ID: u32 = 89;
/// Merit definitions: a [`MERIT_DEFINITION_LEN`]-byte record at `MERIT_DEFINITION_LEN * index`.
pub const MERIT_DEFINITION_FILE_ID: u32 = 88;
/// Merit and merit category names and descriptions on an English install (a Japanese install
/// reads 55566), paired per id ([`crate::dmsg::paired_name_entry`]).
pub const MERIT_STRINGS_FILE_ID_ENGLISH: u32 = 55686;

pub const CATEGORY_RECORDS: usize = 96;
pub const CATEGORY_RECORD_LEN: usize = 12;
const CATEGORY_ID_OFFSET: usize = 0;
const CATEGORY_SECTION_OFFSET: usize = 3;
const CATEGORY_CAP_OFFSET: usize = 4;

pub const MERIT_DEFINITION_LEN: usize = 12;
const DEFINITION_INDEX_OFFSET: usize = 0;
const DEFINITION_MAX_OFFSET: usize = 4;
const DEFINITION_JOB_MASK_OFFSET: usize = 8;

/// Job point categories: [`JOB_CATEGORY_RECORDS`] records of [`JOB_CATEGORY_RECORD_LEN`] bytes,
/// record `j` describing job `j`.
pub const JOB_CATEGORY_FILE_ID: u32 = 94;
/// Job point type definitions: a [`JOB_POINT_TYPE_RECORD_LEN`]-byte record per type id.
pub const JOB_POINT_TYPE_FILE_ID: u32 = 93;
/// Job and job point type names and descriptions on an English install (a Japanese install
/// reads 55574), paired per id.
pub const JOB_POINT_STRINGS_FILE_ID_ENGLISH: u32 = 55694;

pub const JOB_CATEGORY_RECORDS: usize = 32;
pub const JOB_CATEGORY_RECORD_LEN: usize = 68;
/// Job point type ids per job category record, `0` marking an empty slot.
pub const JOB_CATEGORY_TYPE_SLOTS: usize = 32;
const JOB_CATEGORY_JOB_OFFSET: usize = 2;
const JOB_CATEGORY_TYPES_OFFSET: usize = 4;

pub const JOB_POINT_TYPE_RECORD_LEN: usize = 4;
const TYPE_ID_OFFSET: usize = 0;
const TYPE_MAX_OFFSET: usize = 3;

/// Job `j`'s gift thresholds live in file `GIFT_THRESHOLD_FILE_ID_BASE + j`.
pub const GIFT_THRESHOLD_FILE_ID_BASE: u32 = 100_407;
/// Gift slots per job: one u16 threshold (job points spent) each, `0` meaning no gift.
pub const GIFTS_PER_JOB: usize = 256;
/// Gift names and descriptions on an English install (a Japanese install reads 55554).
pub const GIFT_STRINGS_FILE_ID_ENGLISH: u32 = 55674;

const _: () = assert!(
    JOB_CATEGORY_TYPES_OFFSET + 2 * JOB_CATEGORY_TYPE_SLOTS == JOB_CATEGORY_RECORD_LEN,
    "u16 unused, u8 job, u8 unused, u16 type ids[32]"
);

/// One merit category record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MeritCategory {
    /// Byte 0: the category id, which also keys its name.
    pub id: u16,
    /// Byte 3, signed: the section id a category lookup matches.
    pub section: i8,
    /// Byte 4: the upgrade cap, `0` marking an absent record.
    pub cap: u8,
}

impl MeritCategory {
    pub fn is_present(&self) -> bool {
        self.cap != 0
    }
}

/// The category records in file order; a record cut short by the file is dropped.
pub fn parse_categories(bytes: &[u8]) -> Vec<MeritCategory> {
    (0..CATEGORY_RECORDS)
        .map(|record| record * CATEGORY_RECORD_LEN)
        .take_while(|&start| start + CATEGORY_RECORD_LEN <= bytes.len())
        .map(|start| MeritCategory {
            id: u16::from_le_bytes([
                bytes[start + CATEGORY_ID_OFFSET],
                bytes[start + CATEGORY_ID_OFFSET + 1],
            ]),
            section: i8::from_le_bytes([bytes[start + CATEGORY_SECTION_OFFSET]]),
            cap: bytes[start + CATEGORY_CAP_OFFSET],
        })
        .collect()
}

/// One merit's definition record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MeritDefinition {
    /// The most upgrades the merit takes.
    pub max: u8,
    /// Bit `n` set: job `n` may raise it.
    pub job_mask: u32,
}

impl MeritDefinition {
    pub fn allows_job(&self, job: u8) -> bool {
        1u32.checked_shl(u32::from(job))
            .is_some_and(|bit| self.job_mask & bit != 0)
    }
}

/// The definition of merit `index`, `None` when the record lies past the file, names another
/// index, or has no upgrades.
pub fn merit_definition(bytes: &[u8], index: u16) -> Option<MeritDefinition> {
    let start = usize::from(index) * MERIT_DEFINITION_LEN;
    let record = bytes.get(start..start + MERIT_DEFINITION_LEN)?;
    let id = u16::from_le_bytes([
        record[DEFINITION_INDEX_OFFSET],
        record[DEFINITION_INDEX_OFFSET + 1],
    ]);
    let max = record[DEFINITION_MAX_OFFSET];
    if id != index || max == 0 {
        return None;
    }
    let o = DEFINITION_JOB_MASK_OFFSET;
    Some(MeritDefinition {
        max,
        job_mask: u32::from_le_bytes([record[o], record[o + 1], record[o + 2], record[o + 3]]),
    })
}

/// One job point category record: its job (`0` for an empty record) and its type ids in slot
/// order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JobPointCategory {
    pub job: u8,
    pub types: [u16; JOB_CATEGORY_TYPE_SLOTS],
}

/// The job point category records in file order; a record cut short by the file is dropped.
pub fn parse_job_categories(bytes: &[u8]) -> Vec<JobPointCategory> {
    (0..JOB_CATEGORY_RECORDS)
        .map(|record| record * JOB_CATEGORY_RECORD_LEN)
        .take_while(|&start| start + JOB_CATEGORY_RECORD_LEN <= bytes.len())
        .map(|start| {
            let mut types = [0u16; JOB_CATEGORY_TYPE_SLOTS];
            for (slot, type_id) in types.iter_mut().enumerate() {
                let o = start + JOB_CATEGORY_TYPES_OFFSET + 2 * slot;
                *type_id = u16::from_le_bytes([bytes[o], bytes[o + 1]]);
            }
            JobPointCategory {
                job: bytes[start + JOB_CATEGORY_JOB_OFFSET],
                types,
            }
        })
        .collect()
}

/// The max level of job point type `type_id`, `None` when the type has no definition (its
/// record lies past the file or names another id).
pub fn job_point_type_max(bytes: &[u8], type_id: u16) -> Option<u8> {
    let start = usize::from(type_id) * JOB_POINT_TYPE_RECORD_LEN;
    let record = bytes.get(start..start + JOB_POINT_TYPE_RECORD_LEN)?;
    let id = u16::from_le_bytes([record[TYPE_ID_OFFSET], record[TYPE_ID_OFFSET + 1]]);
    (id == type_id).then_some(record[TYPE_MAX_OFFSET])
}

pub fn gift_threshold_file_id(job: u8) -> u32 {
    GIFT_THRESHOLD_FILE_ID_BASE + u32::from(job)
}

/// One job's gift thresholds by gift slot; a file shorter than [`GIFTS_PER_JOB`] slots yields
/// fewer.
pub fn parse_gift_thresholds(bytes: &[u8]) -> Vec<u16> {
    bytes
        .chunks_exact(2)
        .take(GIFTS_PER_JOB)
        .map(|w| u16::from_le_bytes([w[0], w[1]]))
        .collect()
}

/// The [`GIFT_STRINGS_FILE_ID_ENGLISH`] entry naming gift `gift` of job `job` (jobs from 1); its
/// description is the next entry.
pub fn gift_name_entry(job: u8, gift: usize) -> Option<usize> {
    let job_slot = usize::from(job).checked_sub(1)?;
    Some(2 * (GIFTS_PER_JOB * job_slot + gift))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn category(id: u16, section: i8, cap: u8) -> Vec<u8> {
        let mut out = vec![0u8; CATEGORY_RECORD_LEN];
        out[..2].copy_from_slice(&id.to_le_bytes());
        out[CATEGORY_SECTION_OFFSET] = section.to_le_bytes()[0];
        out[CATEGORY_CAP_OFFSET] = cap;
        out
    }

    #[test]
    fn categories_read_id_signed_section_and_cap_in_file_order() {
        let mut bytes = category(0, 0, 0);
        bytes.extend(category(2, 2, 15));
        bytes.extend(category(32, -6, 10));
        bytes.extend(vec![0u8; CATEGORY_RECORD_LEN - 1]);
        let records = parse_categories(&bytes);
        assert_eq!(records.len(), 3, "the truncated fourth record drops");
        assert!(!records[0].is_present());
        assert_eq!(
            records[1],
            MeritCategory {
                id: 2,
                section: 2,
                cap: 15
            }
        );
        assert_eq!(records[2].section, -6, "byte 3 is signed");
        let many: Vec<u8> = (0..CATEGORY_RECORDS + 4)
            .flat_map(|id| category(id as u16, 1, 1))
            .collect();
        assert_eq!(parse_categories(&many).len(), CATEGORY_RECORDS);
    }

    fn definition(index: u16, max: u8, job_mask: u32) -> Vec<u8> {
        let mut out = vec![0u8; MERIT_DEFINITION_LEN];
        out[..2].copy_from_slice(&index.to_le_bytes());
        out[DEFINITION_MAX_OFFSET] = max;
        out[DEFINITION_JOB_MASK_OFFSET..DEFINITION_JOB_MASK_OFFSET + 4]
            .copy_from_slice(&job_mask.to_le_bytes());
        out
    }

    /// The record at `12 * index` defines `index` only when its u16 names that index and its max
    /// is non-zero.
    #[test]
    fn merit_definitions_check_their_index_and_max() {
        const WAR_MASK: u32 = 1 << 1;
        let mut bytes = definition(0, 0, 0);
        bytes.extend(definition(1, 5, WAR_MASK));
        bytes.extend(definition(7, 5, WAR_MASK));
        bytes.extend(definition(3, 0, WAR_MASK));
        let first = merit_definition(&bytes, 1).expect("defined");
        assert_eq!(first.max, 5);
        assert!(first.allows_job(1));
        assert!(!first.allows_job(2));
        assert!(
            !first.allows_job(40),
            "a job past the mask is never allowed"
        );
        assert_eq!(merit_definition(&bytes, 2), None, "names index 7");
        assert_eq!(merit_definition(&bytes, 3), None, "max 0 is absent");
        assert_eq!(merit_definition(&bytes, 4), None, "past the file");
    }

    #[test]
    fn job_categories_read_the_job_and_thirty_two_type_slots() {
        const RDM: u8 = 5;
        let mut bytes = vec![0u8; JOB_CATEGORY_RECORD_LEN * 6];
        let start = usize::from(RDM) * JOB_CATEGORY_RECORD_LEN;
        bytes[start + JOB_CATEGORY_JOB_OFFSET] = RDM;
        for (slot, type_id) in [(0usize, 160u16), (1, 161), (31, 191)] {
            let o = start + JOB_CATEGORY_TYPES_OFFSET + 2 * slot;
            bytes[o..o + 2].copy_from_slice(&type_id.to_le_bytes());
        }
        let records = parse_job_categories(&bytes);
        assert_eq!(records.len(), 6);
        let rdm = records[usize::from(RDM)];
        assert_eq!(rdm.job, RDM, "record j describes job j");
        assert_eq!(&rdm.types[..3], &[160, 161, 0]);
        assert_eq!(rdm.types[JOB_CATEGORY_TYPE_SLOTS - 1], 191);
        assert_eq!(records[0].job, 0);
    }

    #[test]
    fn job_point_type_max_needs_a_matching_record() {
        const TYPE: u16 = 3;
        let mut bytes = vec![0u8; JOB_POINT_TYPE_RECORD_LEN * 4];
        let o = usize::from(TYPE) * JOB_POINT_TYPE_RECORD_LEN;
        bytes[o..o + 2].copy_from_slice(&TYPE.to_le_bytes());
        bytes[o + TYPE_MAX_OFFSET] = 20;
        assert_eq!(job_point_type_max(&bytes, TYPE), Some(20));
        assert_eq!(job_point_type_max(&bytes, 2), None, "names id 0");
        assert_eq!(job_point_type_max(&bytes, 9), None, "past the file");
    }

    #[test]
    fn gifts_read_per_job_files_and_paired_strings() {
        const WAR: u8 = 1;
        const RDM: u8 = 5;
        assert_eq!(
            gift_threshold_file_id(RDM),
            GIFT_THRESHOLD_FILE_ID_BASE + u32::from(RDM)
        );
        let mut bytes = vec![0u8; 2 * (GIFTS_PER_JOB + 3)];
        bytes[2..4].copy_from_slice(&100u16.to_le_bytes());
        let thresholds = parse_gift_thresholds(&bytes);
        assert_eq!(thresholds.len(), GIFTS_PER_JOB);
        assert_eq!(&thresholds[..2], &[0, 100]);
        assert_eq!(gift_name_entry(WAR, 0), Some(0));
        assert_eq!(gift_name_entry(WAR, 3), Some(6));
        assert_eq!(gift_name_entry(RDM, 1), Some(2 * (4 * GIFTS_PER_JOB + 1)));
        assert_eq!(gift_name_entry(0, 1), None);
    }
}
