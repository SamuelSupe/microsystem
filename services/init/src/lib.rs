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
