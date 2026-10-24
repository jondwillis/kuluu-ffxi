use super::*;
use crate::s2c_layout::{
    job_points as job_points_lsb, merit_entry as merit_lsb, miscdata_job_points as record_lsb,
};

/// One `merit_t` of s2c 0x08C GP_SERV_COMMAND_MERIT
/// (vendor/server/src/map/packets/s2c/0x08c_merit.h): the merit's wire id, the cost of its next
/// upgrade and its current upgrade count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MeritWireEntry {
    pub index: u16,
    pub next: u8,
    pub count: u8,
}

/// s2c 0x08C GP_SERV_COMMAND_MERIT: the first `merit_count` entries of `merits[]`
/// (vendor/server/src/map/packets/s2c/0x08c_merit.h PacketData).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeritList {
    pub entries: Vec<MeritWireEntry>,
}

impl MeritList {
    /// `merit_count` u16 and `padding00` u16 lead the body. LSB sizes `merits[]` with a macro the
    /// layout scrape does not read, so the two header offsets are pinned by a test instead.
    pub const MERIT_COUNT_OFFSET: usize = 0x0;
    pub const MERITS_OFFSET: usize = 0x4;
    pub(crate) const ENTRY_LEN: usize = 0x4;
    pub(crate) const INDEX_OFFSET: usize = 0x0;
    pub(crate) const NEXT_OFFSET: usize = 0x2;
    pub(crate) const COUNT_OFFSET: usize = 0x3;

    /// The entries `merit_count` names, as many of them as the body holds.
    pub fn decode(body: &[u8]) -> Result<Self, DecodeError> {
        if body.len() < Self::MERITS_OFFSET {
            return Err(DecodeError::Truncated(Self::MERITS_OFFSET, body.len()));
        }
        let rd16 = |o: usize| u16::from_le_bytes([body[o], body[o + 1]]);
        let count = usize::from(rd16(Self::MERIT_COUNT_OFFSET));
        let entries = (0..count)
            .map(|slot| Self::MERITS_OFFSET + slot * Self::ENTRY_LEN)
            .take_while(|&start| start + Self::ENTRY_LEN <= body.len())
            .map(|start| MeritWireEntry {
                index: rd16(start + Self::INDEX_OFFSET),
                next: body[start + Self::NEXT_OFFSET],
                count: body[start + Self::COUNT_OFFSET],
            })
            .collect();
        Ok(Self { entries })
    }
}

pin_s2c_offset!(MeritList::ENTRY_LEN, merit_lsb::SIZE, "merit_t size");
pin_s2c_offset!(MeritList::INDEX_OFFSET, merit_lsb::INDEX, "merit_t.index");
pin_s2c_offset!(MeritList::NEXT_OFFSET, merit_lsb::NEXT, "merit_t.next");
pin_s2c_offset!(MeritList::COUNT_OFFSET, merit_lsb::COUNT, "merit_t.count");

/// One `jobpoint_t` of s2c 0x08D GP_SERV_COMMAND_JOB_POINTS
/// (vendor/server/src/map/packets/s2c/0x08d_job_points.h): `index:5, job_no:11` in the first
/// u16, `next:10, level:6` in the second.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JobPointWireEntry {
    pub index: u8,
    pub job_no: u16,
    pub next: u16,
    pub level: u8,
}

impl JobPointWireEntry {
    /// Type ids per job: the job's type id is `job_no * 32 + index`, which is the entry's whole
    /// first u16.
    pub const TYPES_PER_JOB: u16 = 1 << JobPointList::JOB_NO_SHIFT;

    pub fn type_id(&self) -> u16 {
        self.job_no * Self::TYPES_PER_JOB + u16::from(self.index)
    }
}

/// s2c 0x08D GP_SERV_COMMAND_JOB_POINTS: every one of its fixed 64 `points[]` slots, empty ones
/// included (vendor/server/src/map/packets/s2c/0x08d_job_points.h PacketData).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobPointList {
    pub entries: Vec<JobPointWireEntry>,
}

impl JobPointList {
    pub const SLOTS: usize = 64;
    pub(crate) const POINTS_OFFSET: usize = 0x0;
    pub(crate) const ENTRY_LEN: usize = 0x4;
    pub(crate) const FIRST_WORD_OFFSET: usize = 0x0;
    pub(crate) const SECOND_WORD_OFFSET: usize = 0x2;
    pub(crate) const INDEX_MASK: u16 = 0x1F;
    pub(crate) const JOB_NO_SHIFT: u32 = 5;
    pub(crate) const JOB_NO_MASK: u16 = 0x7FF;
    pub(crate) const NEXT_MASK: u16 = 0x3FF;
    pub(crate) const LEVEL_SHIFT: u32 = 10;
    pub(crate) const LEVEL_MASK: u16 = 0x3F;
    pub(crate) const SIZE: usize = Self::POINTS_OFFSET + Self::SLOTS * Self::ENTRY_LEN;

    pub fn decode(body: &[u8]) -> Result<Self, DecodeError> {
        if body.len() < Self::SIZE {
            return Err(DecodeError::Truncated(Self::SIZE, body.len()));
        }
        let rd16 = |o: usize| u16::from_le_bytes([body[o], body[o + 1]]);
        let entries = (0..Self::SLOTS)
            .map(|slot| Self::POINTS_OFFSET + slot * Self::ENTRY_LEN)
            .map(|start| {
                let first = rd16(start + Self::FIRST_WORD_OFFSET);
                let second = rd16(start + Self::SECOND_WORD_OFFSET);
                JobPointWireEntry {
                    index: (first & Self::INDEX_MASK) as u8,
                    job_no: (first >> Self::JOB_NO_SHIFT) & Self::JOB_NO_MASK,
                    next: second & Self::NEXT_MASK,
                    level: ((second >> Self::LEVEL_SHIFT) & Self::LEVEL_MASK) as u8,
                }
            })
            .collect();
        Ok(Self { entries })
    }
}

pin_s2c_offset!(
    JobPointList::POINTS_OFFSET,
    job_points_lsb::POINTS,
    "GP_SERV_COMMAND_JOB_POINTS.points"
);
pin_s2c_offset!(
    JobPointList::SLOTS,
    job_points_lsb::POINTS_COUNT,
    "GP_SERV_COMMAND_JOB_POINTS.points length"
);
pin_s2c_offset!(
    JobPointList::ENTRY_LEN,
    job_points_lsb::POINTS_STRIDE,
    "jobpoint_t size"
);
pin_s2c_offset!(
    JobPointList::SIZE,
    job_points_lsb::SIZE,
    "GP_SERV_COMMAND_JOB_POINTS size"
);
pin_s2c_offset!(
    JobPointList::FIRST_WORD_OFFSET,
    job_points_lsb::POINTS_JOB_NO,
    "jobpoint_t.job_no"
);
pin_s2c_offset!(
    JobPointList::SECOND_WORD_OFFSET,
    job_points_lsb::POINTS_LEVEL,
    "jobpoint_t.level"
);
const _: () = {
    use job_points_lsb as lsb;
    assert!(
        lsb::POINTS_INDEX == JobPointList::FIRST_WORD_OFFSET
            && lsb::POINTS_INDEX_SHIFT == 0
            && lsb::POINTS_INDEX_MASK == JobPointList::INDEX_MASK as u32,
        "LSB moved jobpoint_t.index: the decoder bits are stale"
    );
    assert!(
        lsb::POINTS_JOB_NO_SHIFT == JobPointList::JOB_NO_SHIFT
            && lsb::POINTS_JOB_NO_MASK == JobPointList::JOB_NO_MASK as u32,
        "LSB moved jobpoint_t.job_no: the decoder bits are stale"
    );
    assert!(
        lsb::POINTS_NEXT == JobPointList::SECOND_WORD_OFFSET
            && lsb::POINTS_NEXT_SHIFT == 0
            && lsb::POINTS_NEXT_MASK == JobPointList::NEXT_MASK as u32,
        "LSB moved jobpoint_t.next: the decoder bits are stale"
    );
    assert!(
        lsb::POINTS_LEVEL_SHIFT == JobPointList::LEVEL_SHIFT
            && lsb::POINTS_LEVEL_MASK == JobPointList::LEVEL_MASK as u32,
        "LSB moved jobpoint_t.level: the decoder bits are stale"
    );
};

/// `GP_SERV_COMMAND_MISCDATA_TYPE::JobPoints` (vendor/server/src/map/packets/s2c/0x063_miscdata.h).
pub const MISCDATA_TYPE_JOB_POINTS: u16 = 0x05;

/// One `JobPointData` of an s2c 0x063 JobPoints record
/// (vendor/server/src/map/packets/s2c/0x063_miscdata_job_points.h).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct JobPointsJob {
    pub capacity_points: u16,
    pub current_jp: u16,
    pub total_jp_spent: u16,
}

/// One s2c 0x063 MISCDATA record of type JobPoints: the `access` byte and `jobs[24]`, indexed by
/// job (vendor/server/src/map/packets/s2c/0x063_miscdata_job_points.h PacketData).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MiscdataJobPoints {
    pub access: u8,
    pub jobs: [JobPointsJob; Self::JOBS],
}

impl MiscdataJobPoints {
    pub const JOBS: usize = 24;
    pub(crate) const TYPE_OFFSET: usize = 0x0;
    pub(crate) const ACCESS_OFFSET: usize = 0x4;
    pub(crate) const JOBS_OFFSET: usize = 0x8;
    pub(crate) const JOB_LEN: usize = 0x6;
    pub(crate) const CAPACITY_POINTS_OFFSET: usize = 0x0;
    pub(crate) const CURRENT_JP_OFFSET: usize = 0x2;
    pub(crate) const TOTAL_JP_SPENT_OFFSET: usize = 0x4;
    pub(crate) const SIZE: usize = Self::JOBS_OFFSET + Self::JOBS * Self::JOB_LEN;

    /// `None` when the record is not of type JobPoints.
    pub fn decode(body: &[u8]) -> Option<Result<Self, DecodeError>> {
        let kind = body.get(Self::TYPE_OFFSET..Self::TYPE_OFFSET + 2)?;
        if u16::from_le_bytes([kind[0], kind[1]]) != MISCDATA_TYPE_JOB_POINTS {
            return None;
        }
        if body.len() < Self::SIZE {
            return Some(Err(DecodeError::Truncated(Self::SIZE, body.len())));
        }
        let rd16 = |o: usize| u16::from_le_bytes([body[o], body[o + 1]]);
        let mut jobs = [JobPointsJob::default(); Self::JOBS];
        for (job, entry) in jobs.iter_mut().enumerate() {
            let start = Self::JOBS_OFFSET + job * Self::JOB_LEN;
            *entry = JobPointsJob {
                capacity_points: rd16(start + Self::CAPACITY_POINTS_OFFSET),
                current_jp: rd16(start + Self::CURRENT_JP_OFFSET),
                total_jp_spent: rd16(start + Self::TOTAL_JP_SPENT_OFFSET),
            };
        }
        Some(Ok(Self {
            access: body[Self::ACCESS_OFFSET],
            jobs,
        }))
    }
}

pin_s2c_offset!(
    MiscdataJobPoints::TYPE_OFFSET,
    record_lsb::TYPE,
    "GP_SERV_COMMAND_MISCDATA::JOB_POINTS.type"
);
pin_s2c_offset!(
    MiscdataJobPoints::ACCESS_OFFSET,
    record_lsb::ACCESS,
    "GP_SERV_COMMAND_MISCDATA::JOB_POINTS.access"
);
pin_s2c_offset!(
    MiscdataJobPoints::JOBS_OFFSET,
    record_lsb::JOBS,
    "GP_SERV_COMMAND_MISCDATA::JOB_POINTS.jobs"
);
pin_s2c_offset!(
    MiscdataJobPoints::JOBS,
    record_lsb::JOBS_COUNT,
    "GP_SERV_COMMAND_MISCDATA::JOB_POINTS.jobs length"
);
pin_s2c_offset!(
    MiscdataJobPoints::JOB_LEN,
    record_lsb::JOBS_STRIDE,
    "JobPointData size"
);
pin_s2c_offset!(
    MiscdataJobPoints::JOBS_OFFSET + MiscdataJobPoints::CAPACITY_POINTS_OFFSET,
    record_lsb::JOBS_CAPACITY_POINTS,
    "JobPointData.capacityPoints"
);
pin_s2c_offset!(
    MiscdataJobPoints::JOBS_OFFSET + MiscdataJobPoints::CURRENT_JP_OFFSET,
    record_lsb::JOBS_CURRENT_JP,
    "JobPointData.currentJp"
);
pin_s2c_offset!(
    MiscdataJobPoints::JOBS_OFFSET + MiscdataJobPoints::TOTAL_JP_SPENT_OFFSET,
    record_lsb::JOBS_TOTAL_JP_SPENT,
    "JobPointData.totalJpSpent"
);
pin_s2c_offset!(
    MiscdataJobPoints::SIZE,
    record_lsb::SIZE,
    "GP_SERV_COMMAND_MISCDATA::JOB_POINTS size"
);

#[cfg(test)]
mod tests {
    use super::*;

    fn merit_body(count: u16, entries: &[(u16, u8, u8)]) -> Vec<u8> {
        let mut body = vec![0u8; MeritList::MERITS_OFFSET];
        body[..2].copy_from_slice(&count.to_le_bytes());
        for &(index, next, count) in entries {
            body.extend_from_slice(&index.to_le_bytes());
            body.push(next);
            body.push(count);
        }
        body
    }

    /// LSB's header puts `merit_count` and `padding00` (two u16) ahead of `merits[]`
    /// (vendor/server/src/map/packets/s2c/0x08c_merit.h PacketData); the scrape cannot lay that
    /// struct out, so its two offsets are pinned here.
    #[test]
    fn merit_list_header_is_two_u16_ahead_of_the_entries() {
        assert_eq!(MeritList::MERIT_COUNT_OFFSET, 0);
        assert_eq!(MeritList::MERITS_OFFSET, 2 * std::mem::size_of::<u16>());
    }

    /// LSB's single-merit update sends `merit_count = 1` with two entries in the body; only the
    /// counted one is read.
    #[test]
    fn merit_list_reads_merit_count_entries() {
        const HP_WIRE_ID: u16 = 0x40;
        const MP_WIRE_ID: u16 = 0x42;
        let body = merit_body(1, &[(HP_WIRE_ID, 3, 2), (MP_WIRE_ID, 1, 0)]);
        let list = MeritList::decode(&body).expect("decode");
        assert_eq!(
            list.entries,
            vec![MeritWireEntry {
                index: HP_WIRE_ID,
                next: 3,
                count: 2
            }]
        );

        let body = merit_body(5, &[(HP_WIRE_ID, 3, 2), (MP_WIRE_ID, 1, 0)]);
        assert_eq!(
            MeritList::decode(&body).unwrap().entries.len(),
            2,
            "a count past the body stops at the body"
        );
        assert!(matches!(
            MeritList::decode(&body[..MeritList::MERITS_OFFSET - 1]),
            Err(DecodeError::Truncated(..))
        ));
    }

    #[test]
    fn job_point_list_splits_both_words_by_lsb_bitfields() {
        const RDM: u16 = 5;
        const TYPE_SLOT: u8 = 3;
        const NEXT: u16 = 0x2AB;
        const LEVEL: u8 = 17;
        let mut body = vec![0u8; JobPointList::SIZE];
        let first = (RDM << JobPointList::JOB_NO_SHIFT) | u16::from(TYPE_SLOT);
        let second = (u16::from(LEVEL) << JobPointList::LEVEL_SHIFT) | NEXT;
        let slot = 2 * JobPointList::ENTRY_LEN;
        body[slot..slot + 2].copy_from_slice(&first.to_le_bytes());
        body[slot + 2..slot + 4].copy_from_slice(&second.to_le_bytes());
        let list = JobPointList::decode(&body).expect("decode");
        assert_eq!(list.entries.len(), JobPointList::SLOTS);
        let entry = list.entries[2];
        assert_eq!(
            entry,
            JobPointWireEntry {
                index: TYPE_SLOT,
                job_no: RDM,
                next: NEXT,
                level: LEVEL
            }
        );
        assert_eq!(
            entry.type_id(),
            first,
            "the type id is the whole first word"
        );
        assert_eq!(list.entries[0].type_id(), 0);
        assert!(matches!(
            JobPointList::decode(&body[..JobPointList::SIZE - 1]),
            Err(DecodeError::Truncated(..))
        ));
    }

    #[test]
    fn miscdata_job_points_reads_access_and_every_job() {
        const WAR: usize = 1;
        const RUN: usize = 22;
        let mut body = vec![0u8; record_lsb::SIZE];
        body[..2].copy_from_slice(&MISCDATA_TYPE_JOB_POINTS.to_le_bytes());
        body[record_lsb::ACCESS] = 1;
        for (job, (cp, jp, spent)) in [(WAR, (12_345u16, 499u16, 2100u16)), (RUN, (7, 500, 1))] {
            let start = record_lsb::JOBS + job * record_lsb::JOBS_STRIDE;
            body[start..start + 2].copy_from_slice(&cp.to_le_bytes());
            body[start + 2..start + 4].copy_from_slice(&jp.to_le_bytes());
            body[start + 4..start + 6].copy_from_slice(&spent.to_le_bytes());
        }
        let record = MiscdataJobPoints::decode(&body)
            .expect("type 5")
            .expect("decode");
        assert_eq!(record.access, 1);
        assert_eq!(
            record.jobs[WAR],
            JobPointsJob {
                capacity_points: 12_345,
                current_jp: 499,
                total_jp_spent: 2100
            }
        );
        assert_eq!(record.jobs[RUN].current_jp, 500);
        assert_eq!(record.jobs[0], JobPointsJob::default());

        assert!(matches!(
            MiscdataJobPoints::decode(&body[..MiscdataJobPoints::SIZE - 1]),
            Some(Err(DecodeError::Truncated(..)))
        ));
        body[..2].copy_from_slice(&MISCDATA_TYPE_UNITY.to_le_bytes());
        assert!(
            MiscdataJobPoints::decode(&body).is_none(),
            "Unity is not JobPoints"
        );
    }

    /// The record keys job point type state by `job_no * 32 + index`
    /// (.agents/skills/retail-observe/references/2026-10-15-merit-and-job-point-windows.md).
    #[test]
    fn type_ids_run_thirty_two_per_job() {
        const RECORD_PINNED_TYPES_PER_JOB: u16 = 32;
        assert_eq!(
            JobPointWireEntry::TYPES_PER_JOB,
            RECORD_PINNED_TYPES_PER_JOB
        );
    }
}
