#![no_std]

use microsystem_abi::{ObjectType, Rights};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Criticality {
    Critical,
    Restartable,
    Application,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Grant {
    pub object_type: ObjectType,
    pub rights: Rights,
    pub count: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ServiceSpec {
    pub name: &'static str,
    pub heap_pages: u32,
    pub stack_pages: u16,
    pub criticality: Criticality,
    pub grants: &'static [Grant],
}

const CONSOLE_GRANTS: &[Grant] = &[
    Grant {
        object_type: ObjectType::MmioRegion,
        rights: Rights(Rights::READ.0 | Rights::WRITE.0 | Rights::MAP.0),
        count: 1,
    },
    Grant {
        object_type: ObjectType::Irq,
        rights: Rights(Rights::ACK.0 | Rights::MANAGE.0),
        count: 1,
    },
];
const BLOCK_GRANTS: &[Grant] = &[
    Grant {
        object_type: ObjectType::MmioRegion,
        rights: Rights(Rights::READ.0 | Rights::WRITE.0 | Rights::MAP.0),
        count: 1,
    },
    Grant {
        object_type: ObjectType::DmaDomain,
        rights: Rights(Rights::READ.0 | Rights::WRITE.0 | Rights::MAP.0),
        count: 1,
    },
    Grant {
        object_type: ObjectType::Irq,
        rights: Rights(Rights::ACK.0 | Rights::MANAGE.0),
        count: 1,
    },
];

pub const BOOT_SERVICES: &[ServiceSpec] = &[
    ServiceSpec {
        name: "console",
        heap_pages: 16,
        stack_pages: 4,
        criticality: Criticality::Critical,
        grants: CONSOLE_GRANTS,
    },
    ServiceSpec {
        name: "block",
        heap_pages: 64,
        stack_pages: 8,
        criticality: Criticality::Critical,
        grants: BLOCK_GRANTS,
    },
    ServiceSpec {
        name: "mfs",
        heap_pages: 8192,
        stack_pages: 8,
        criticality: Criticality::Critical,
        grants: &[],
    },
    ServiceSpec {
        name: "shell",
        heap_pages: 32,
        stack_pages: 4,
        criticality: Criticality::Restartable,
        grants: &[],
    },
];

pub fn retry_delay(attempt: u8) -> u64 {
    (1u64 << attempt.saturating_sub(1).min(5)).min(30) * 1_000_000_000
}

pub fn dependencies_ready(task: usize, infos: &[microsystem_abi::service::InfoV1; 13]) -> bool {
    use microsystem_abi::service::{ONLINE, STARTING};
    let online = |index: usize| infos[index].state == ONLINE;
    match task {
        2 => matches!(infos[5].state, STARTING | ONLINE),
        3 => online(2),
        12 | 6 | 11 => online(3),
        4 => online(1),
        10 => online(11),
        7..=9 => matches!(infos[6].state, STARTING | ONLINE),
        _ => true,
    }
}

pub fn depends_on(task: usize, parent: usize) -> bool {
    match parent {
        1 => task == 4,
        5 => matches!(task, 2 | 3 | 12 | 6..=11),
        2 => matches!(task, 3 | 12 | 6..=11),
        3 => matches!(task, 12 | 6..=11),
        6 => matches!(task, 7..=9),
        11 => task == 10,
        _ => false,
    }
}

#[cfg(test)]
mod recovery_tests {
    use super::*;
    use microsystem_abi::service;

    #[test]
    fn consumers_wait_for_storage_replay_and_gui_registration_can_break_its_boot_cycle() {
        let mut infos = [service::InfoV1::EMPTY; 13];
        infos[5].state = service::STARTING;
        assert!(dependencies_ready(2, &infos));
        assert!(!dependencies_ready(3, &infos));
        infos[2].state = service::ONLINE;
        assert!(dependencies_ready(3, &infos));
        assert!(!dependencies_ready(6, &infos));
        assert!(!dependencies_ready(12, &infos));
        assert!(!dependencies_ready(11, &infos));
        infos[3].state = service::ONLINE;
        assert!(dependencies_ready(6, &infos));
        assert!(dependencies_ready(12, &infos));
        assert!(dependencies_ready(11, &infos));
        infos[6].state = service::STARTING;
        assert!(dependencies_ready(7, &infos));
        assert!(dependencies_ready(8, &infos));
        infos[2].state = service::QUIESCE_FAILED;
        assert!(!dependencies_ready(3, &infos));
        assert!(depends_on(9, 5));
        assert!(!depends_on(4, 5));
        assert!(depends_on(10, 11));
        assert!(depends_on(11, 3));
        assert!(depends_on(10, 5));
    }

    #[test]
    fn recovery_backoff_is_bounded_under_repeated_failures() {
        for (attempt, seconds) in [1, 2, 4, 8, 16, 30, 30, 30].into_iter().enumerate() {
            assert_eq!(retry_delay(attempt as u8 + 1), seconds * 1_000_000_000);
        }
        assert_eq!(retry_delay(u8::MAX), 30_000_000_000);
    }
}
