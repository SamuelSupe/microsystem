use microsystem_abi::{CapHandle, Message, ObjectType, Rights, Status};
use microsystem_kernel::capability::{CapabilityTable, MAX_CAPABILITIES};
use microsystem_kernel::elf::{EM_AARCH64, ET_EXEC, ElfImage, PF_W, PF_X, USER_MIN};
use microsystem_kernel::ipc::{EndpointQueue, Envelope};
use microsystem_kernel::memory::{FrameAllocator, Mapping, PAGE_SIZE};
use microsystem_kernel::scheduler::{Scheduler, ThreadState};

fn rights(bits: &[Rights]) -> Rights {
    Rights(bits.iter().fold(0, |value, right| value | right.0))
}

#[test]
fn capability_generation_blocks_stale_handles_and_limits_rights() {
    let mut table = CapabilityTable::new();
    let root = table
        .insert_root(
            42,
            ObjectType::Frame,
            rights(&[Rights::READ, Rights::WRITE, Rights::GRANT, Rights::MANAGE]),
        )
        .unwrap();
    let read_only = table.copy_cap(root, Rights::READ).unwrap();
    assert_eq!(table.lookup(read_only, Rights::READ).unwrap().object, 42);
    assert_eq!(
        table.copy_cap(read_only, Rights::WRITE),
        Err(Status::AccessDenied)
    );

    table.delete(read_only).unwrap();
    assert_eq!(
        table.lookup(read_only, Rights::READ),
        Err(Status::BadCapability)
    );
    let recycled = table
        .insert_root(43, ObjectType::Frame, Rights::READ)
        .unwrap();
    assert_eq!(recycled.slot(), read_only.slot());
    assert_ne!(recycled.generation(), read_only.generation());
    assert_eq!(
        table.lookup(read_only, Rights::READ),
        Err(Status::BadCapability)
    );
}

#[test]
fn revoke_does_not_cross_generation_reuse_into_old_descendants() {
    let mut table = CapabilityTable::new();
    let old_parent = table
        .insert_root(
            7,
            ObjectType::Endpoint,
            rights(&[Rights::READ, Rights::GRANT, Rights::MANAGE]),
        )
        .unwrap();
    let old_child = table.copy_cap(old_parent, Rights::READ).unwrap();

    table.delete(old_parent).unwrap();
    let new_parent = table
        .insert_root(
            8,
            ObjectType::Endpoint,
            rights(&[Rights::READ, Rights::GRANT, Rights::MANAGE]),
        )
        .unwrap();
    assert_eq!(new_parent.slot(), old_parent.slot());
    assert_ne!(new_parent.generation(), old_parent.generation());

    table.revoke(new_parent).unwrap();
    assert!(table.lookup(old_child, Rights::READ).is_ok());
    assert!(table.lookup(new_parent, Rights::MANAGE).is_ok());
}

#[test]
fn revoke_removes_the_entire_derived_capability_tree() {
    let mut table = CapabilityTable::new();
    let root = table
        .insert_root(
            7,
            ObjectType::Endpoint,
            rights(&[Rights::READ, Rights::GRANT, Rights::MANAGE]),
        )
        .unwrap();
    let child = table
        .copy_cap(root, rights(&[Rights::READ, Rights::GRANT]))
        .unwrap();
    let grandchild = table.copy_cap(child, Rights::READ).unwrap();
    assert_eq!(table.revoke(root).unwrap(), 2);
    assert!(table.lookup(root, Rights::MANAGE).is_ok());
    assert_eq!(
        table.lookup(child, Rights::READ),
        Err(Status::BadCapability)
    );
    assert_eq!(
        table.lookup(grandchild, Rights::READ),
        Err(Status::BadCapability)
    );
    assert_eq!(table.len(), 1);
}

#[test]
fn cross_table_derivation_survives_generation_reuse_and_global_revoke() {
    let mut sender = CapabilityTable::new();
    let mut receiver = CapabilityTable::new();
    let root = sender
        .insert_root_with_node(
            100,
            ObjectType::Endpoint,
            rights(&[Rights::READ, Rights::GRANT, Rights::MANAGE]),
            0x100,
        )
        .unwrap();
    let exported = sender.capability(root, Rights::GRANT).unwrap();
    let imported = receiver.import_copy(exported, 0x200).unwrap();
    let child = receiver
        .copy_cap_with_node(imported, Rights::READ, 0x201)
        .unwrap();

    sender.delete(root).unwrap();
    let replacement = sender
        .insert_root_with_node(
            101,
            ObjectType::Endpoint,
            rights(&[Rights::READ, Rights::GRANT, Rights::MANAGE]),
            0x300,
        )
        .unwrap();
    assert_eq!(replacement.slot(), root.slot());
    assert_ne!(replacement.generation(), root.generation());
    assert_eq!(receiver.lookup(imported, Rights::READ).unwrap().object, 100);

    // A cross-table revoke walks stable derivation nodes, not slot handles.
    let mut nodes = [0u32; 4];
    nodes[0] = exported.node;
    let mut node_count = 1;
    loop {
        let mut changed = false;
        changed |= sender.collect_child_nodes(&mut nodes, &mut node_count);
        changed |= receiver.collect_child_nodes(&mut nodes, &mut node_count);
        if !changed {
            break;
        }
    }
    assert_eq!(&nodes[..node_count], &[0x100, 0x200, 0x201]);
    sender.delete_nodes(&nodes[1..node_count]);
    receiver.delete_nodes(&nodes[1..node_count]);
    assert_eq!(
        receiver.lookup(imported, Rights::READ),
        Err(Status::BadCapability)
    );
    assert_eq!(
        receiver.lookup(child, Rights::READ),
        Err(Status::BadCapability)
    );
    assert!(sender.lookup(replacement, Rights::MANAGE).is_ok());
}

#[test]
fn move_failure_when_destination_is_full_is_atomic() {
    let mut table = CapabilityTable::new();
    let source = table
        .insert_root(1, ObjectType::Frame, Rights::READ)
        .unwrap();
    for object in 2..=(MAX_CAPABILITIES as u32 - 1) {
        table
            .insert_root(object, ObjectType::Frame, Rights::READ)
            .unwrap();
    }
    assert_eq!(table.free_slots(), 0);
    assert_eq!(table.move_cap(source), Err(Status::NoMemory));
    assert_eq!(table.lookup(source, Rights::READ).unwrap().object, 1);
}

#[test]
fn allocator_respects_reservations_and_double_release() {
    let mut allocator = FrameAllocator::empty();
    allocator.initialize(0x1000_0000, PAGE_SIZE * 4).unwrap();
    allocator.reserve(0x1000_1000, PAGE_SIZE).unwrap();
    assert_eq!(allocator.allocate().unwrap(), 0x1000_0000);
    assert_eq!(allocator.allocate().unwrap(), 0x1000_2000);
    assert_eq!(allocator.free_frames(), 1);
    assert_eq!(
        allocator.release(0x1000_0000),
        Ok(()),
        "allocated frames may be returned"
    );
    assert_eq!(allocator.release(0x1000_0000), Err(Status::Invalid));
    assert_eq!(allocator.release(0x1000_1000), Err(Status::Invalid));
}

#[test]
fn mapping_validation_enforces_alignment_and_w_x() {
    let valid = Mapping {
        virtual_address: 0x4000,
        physical_address: 0x8000,
        writable: true,
        executable: false,
        user: true,
    };
    assert_eq!(valid.validate().unwrap(), valid);
    assert_eq!(
        Mapping {
            executable: true,
            ..valid
        }
        .validate(),
        Err(Status::AccessDenied)
    );
    assert_eq!(
        Mapping {
            virtual_address: 0x4001,
            ..valid
        }
        .validate(),
        Err(Status::Invalid)
    );
}

#[test]
fn elf_loader_rejects_wx_segments_sharing_one_page() {
    let mut image = vec![0u8; 0x140];
    image[0..4].copy_from_slice(b"\x7fELF");
    image[4] = 2;
    image[5] = 1;
    put_u16(&mut image, 16, ET_EXEC);
    put_u16(&mut image, 18, EM_AARCH64);
    put_u64(&mut image, 24, USER_MIN);
    put_u64(&mut image, 32, 64);
    put_u16(&mut image, 54, 56);
    put_u16(&mut image, 56, 2);

    // The byte ranges are disjoint, but both segments resolve to page
    // 0x0040_0000. Mapping them independently could let the second segment
    // replace the first page's execute permission with write permission.
    put_load_segment(&mut image, 64, 0x100, USER_MIN, 0x10, PF_X);
    put_load_segment(&mut image, 120, 0x120, USER_MIN + 0x10, 0x10, PF_W);

    assert!(matches!(ElfImage::parse(&image), Err(Status::Invalid)));
}

fn put_load_segment(
    image: &mut [u8],
    offset: usize,
    file_offset: u64,
    virtual_address: u64,
    size: u64,
    flags: u32,
) {
    put_u32(image, offset, 1);
    put_u32(image, offset + 4, flags);
    put_u64(image, offset + 8, file_offset);
    put_u64(image, offset + 16, virtual_address);
    put_u64(image, offset + 32, size);
    put_u64(image, offset + 40, size);
}

fn put_u16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn put_u64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

#[test]
fn scheduler_preempts_and_wakes_threads_without_losing_fifo_order() {
    let mut scheduler = Scheduler::new();
    let first = scheduler.create(1).unwrap();
    let second = scheduler.create(2).unwrap();
    assert_eq!(scheduler.schedule(0), Some(first));
    assert_eq!(scheduler.thread(first).unwrap().state, ThreadState::Running);
    scheduler.preempt(first).unwrap();
    assert_eq!(scheduler.schedule(1), Some(second));
    scheduler.block(second).unwrap();
    assert_eq!(scheduler.schedule(0), Some(first));
    scheduler.block(first).unwrap();
    assert_eq!(scheduler.schedule(0), None);
    scheduler.wake(second).unwrap();
    assert_eq!(scheduler.schedule(0), Some(second));
}

#[test]
fn endpoint_queue_is_fifo_and_has_a_hard_capacity_bound() {
    let mut queue = EndpointQueue::<2>::new();
    let first = Envelope {
        sender: 1,
        message: Message::new(1, 1),
    };
    let second = Envelope {
        sender: 2,
        message: Message::new(1, 2),
    };
    queue.send(first).unwrap();
    queue.send(second).unwrap();
    assert_eq!(queue.send(first), Err(Status::Busy));
    assert_eq!(queue.receive(), Some(first));
    assert_eq!(queue.receive(), Some(second));
    assert_eq!(queue.receive(), None);
}

#[test]
fn zero_capacity_endpoint_cannot_accept_messages() {
    let mut queue = EndpointQueue::<0>::new();
    let envelope = Envelope {
        sender: 1,
        message: Message::new(1, 1),
    };
    assert_eq!(queue.send(envelope), Err(Status::Busy));
    assert!(queue.receive().is_none());
    assert_eq!(CapHandle::INVALID.generation(), 0);
}
