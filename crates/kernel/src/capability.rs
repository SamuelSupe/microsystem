use microsystem_abi::{CapHandle, ObjectType, Rights, Status};

pub const MAX_CAPABILITIES: usize = 256;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Capability {
    pub object: u32,
    pub object_type: ObjectType,
    pub rights: Rights,
    pub node: u32,
    pub parent_node: u32,
}

#[derive(Clone, Copy)]
struct Slot {
    generation: u16,
    value: Option<Capability>,
}

impl Slot {
    const EMPTY: Self = Self {
        generation: 1,
        value: None,
    };
}

pub struct CapabilityTable {
    slots: [Slot; MAX_CAPABILITIES],
    next_node: u32,
}

impl Default for CapabilityTable {
    fn default() -> Self {
        Self::new()
    }
}

impl CapabilityTable {
    pub const fn new() -> Self {
        Self {
            slots: [Slot::EMPTY; MAX_CAPABILITIES],
            next_node: 1,
        }
    }

    pub fn insert_root(
        &mut self,
        object: u32,
        object_type: ObjectType,
        rights: Rights,
    ) -> Result<CapHandle, Status> {
        let node = self.allocate_node();
        self.insert(Capability {
            object,
            object_type,
            rights,
            node,
            parent_node: 0,
        })
    }

    pub fn insert_root_with_node(
        &mut self,
        object: u32,
        object_type: ObjectType,
        rights: Rights,
        node: u32,
    ) -> Result<CapHandle, Status> {
        if node == 0 || self.contains_node(node) {
            return Err(Status::Invalid);
        }
        let handle = self.insert(Capability {
            object,
            object_type,
            rights,
            node,
            parent_node: 0,
        })?;
        self.observe_node(node);
        Ok(handle)
    }

    pub fn insert_root_at(
        &mut self,
        handle: CapHandle,
        object: u32,
        object_type: ObjectType,
        rights: Rights,
        node: u32,
    ) -> Result<(), Status> {
        let index = handle.slot();
        if handle == CapHandle::INVALID
            || index >= MAX_CAPABILITIES
            || handle.generation() == 0
            || self.slots[index].value.is_some()
            || self.contains_node(node)
        {
            return Err(Status::Invalid);
        }
        self.slots[index].generation = handle.generation();
        self.slots[index].value = Some(Capability {
            object,
            object_type,
            rights,
            node,
            parent_node: 0,
        });
        self.observe_node(node);
        Ok(())
    }

    pub fn lookup(&self, handle: CapHandle, required: Rights) -> Result<&Capability, Status> {
        let slot = self.slot(handle)?;
        let capability = slot.value.as_ref().ok_or(Status::BadCapability)?;
        if !capability.rights.contains(required) {
            return Err(Status::AccessDenied);
        }
        Ok(capability)
    }

    pub fn copy_cap(&mut self, source: CapHandle, rights: Rights) -> Result<CapHandle, Status> {
        let source_cap = *self.lookup(source, Rights::GRANT)?;
        if !source_cap.rights.contains(rights) {
            return Err(Status::AccessDenied);
        }
        let node = self.allocate_node();
        let handle = self.insert(Capability {
            rights,
            node,
            parent_node: source_cap.node,
            ..source_cap
        })?;
        self.observe_node(node);
        Ok(handle)
    }

    pub fn copy_cap_with_node(
        &mut self,
        source: CapHandle,
        rights: Rights,
        node: u32,
    ) -> Result<CapHandle, Status> {
        let source_cap = *self.lookup(source, Rights::GRANT)?;
        if node == 0 || self.contains_node(node) {
            return Err(Status::Invalid);
        }
        if !source_cap.rights.contains(rights) {
            return Err(Status::AccessDenied);
        }
        self.insert(Capability {
            rights,
            node,
            parent_node: source_cap.node,
            ..source_cap
        })
    }

    pub fn import_copy(&mut self, source: Capability, node: u32) -> Result<CapHandle, Status> {
        if node == 0 || self.contains_node(node) {
            return Err(Status::Invalid);
        }
        let handle = self.insert(Capability {
            node,
            parent_node: source.node,
            ..source
        })?;
        self.observe_node(node);
        Ok(handle)
    }

    pub fn import_copy_at(
        &mut self,
        handle: CapHandle,
        source: Capability,
        rights: Rights,
        node: u32,
    ) -> Result<(), Status> {
        if node == 0 || self.contains_node(node) || !source.rights.contains(rights) {
            return Err(Status::AccessDenied);
        }
        self.insert_at(
            handle,
            Capability {
                rights,
                node,
                parent_node: source.node,
                ..source
            },
        )?;
        self.observe_node(node);
        Ok(())
    }

    pub fn import_move(&mut self, source: Capability) -> Result<CapHandle, Status> {
        if source.node == 0 || self.contains_node(source.node) {
            return Err(Status::Invalid);
        }
        let handle = self.insert(source)?;
        self.observe_node(source.node);
        Ok(handle)
    }

    pub fn move_cap(&mut self, source: CapHandle) -> Result<CapHandle, Status> {
        let source_index = self.validate_index(source)?;
        let capability = self.slots[source_index]
            .value
            .ok_or(Status::BadCapability)?;
        let destination = self.insert(capability)?;
        self.clear_slot(source_index);
        Ok(destination)
    }

    pub fn delete(&mut self, handle: CapHandle) -> Result<(), Status> {
        let index = self.validate_index(handle)?;
        if self.slots[index].value.is_none() {
            return Err(Status::BadCapability);
        }
        self.clear_slot(index);
        Ok(())
    }

    pub fn revoke(&mut self, handle: CapHandle) -> Result<usize, Status> {
        let root = *self.lookup(handle, Rights::MANAGE)?;
        let mut marked = [0u32; MAX_CAPABILITIES];
        marked[0] = root.node;
        let mut marked_len = 1;
        let mut changed = true;
        while changed {
            changed = false;
            for capability in self.slots.iter().filter_map(|slot| slot.value) {
                if capability.node != root.node
                    && marked[..marked_len].contains(&capability.parent_node)
                    && !marked[..marked_len].contains(&capability.node)
                {
                    marked[marked_len] = capability.node;
                    marked_len += 1;
                    changed = true;
                }
            }
        }
        Ok(self.delete_nodes(&marked[1..marked_len]))
    }

    pub fn capability(&self, handle: CapHandle, required: Rights) -> Result<Capability, Status> {
        self.lookup(handle, required).copied()
    }

    pub fn free_slots(&self) -> usize {
        self.slots
            .iter()
            .enumerate()
            .skip(1)
            .filter(|(_, slot)| slot.value.is_none())
            .count()
    }

    pub fn handle_for_object_with_rights(
        &self,
        object: u32,
        object_type: ObjectType,
        rights: Rights,
    ) -> Option<CapHandle> {
        self.slots
            .iter()
            .enumerate()
            .skip(1)
            .find_map(|(index, slot)| {
                let capability = slot.value?;
                (capability.object == object
                    && capability.object_type == object_type
                    && capability.rights.contains(rights))
                .then(|| CapHandle::from_parts(index as u16, slot.generation))
            })
    }

    pub fn object_references(&self, object: u32, object_type: ObjectType) -> usize {
        self.slots
            .iter()
            .filter(|slot| {
                slot.value.is_some_and(|capability| {
                    capability.object == object && capability.object_type == object_type
                })
            })
            .count()
    }

    pub fn capabilities(&self) -> impl Iterator<Item = Capability> + '_ {
        self.slots.iter().filter_map(|slot| slot.value)
    }

    pub fn collect_child_nodes(&self, output: &mut [u32], output_len: &mut usize) -> bool {
        let mut changed = false;
        let parent_len = *output_len;
        for capability in self.slots.iter().filter_map(|slot| slot.value) {
            if output[..parent_len].contains(&capability.parent_node)
                && !output[..*output_len].contains(&capability.node)
            {
                if *output_len == output.len() {
                    break;
                }
                output[*output_len] = capability.node;
                *output_len += 1;
                changed = true;
            }
        }
        changed
    }

    pub fn delete_nodes(&mut self, nodes: &[u32]) -> usize {
        let mut removed = 0;
        for index in 1..MAX_CAPABILITIES {
            if self.slots[index]
                .value
                .is_some_and(|capability| nodes.contains(&capability.node))
            {
                self.clear_slot(index);
                removed += 1;
            }
        }
        removed
    }

    pub fn len(&self) -> usize {
        self.slots
            .iter()
            .filter(|slot| slot.value.is_some())
            .count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn insert(&mut self, capability: Capability) -> Result<CapHandle, Status> {
        for (index, slot) in self.slots.iter_mut().enumerate().skip(1) {
            if slot.value.is_none() {
                slot.value = Some(capability);
                return Ok(CapHandle::from_parts(index as u16, slot.generation));
            }
        }
        Err(Status::NoMemory)
    }

    fn insert_at(&mut self, handle: CapHandle, capability: Capability) -> Result<(), Status> {
        let index = handle.slot();
        if handle == CapHandle::INVALID
            || index >= MAX_CAPABILITIES
            || handle.generation() == 0
            || self.slots[index].value.is_some()
        {
            return Err(Status::Invalid);
        }
        self.slots[index].generation = handle.generation();
        self.slots[index].value = Some(capability);
        Ok(())
    }

    fn contains_node(&self, node: u32) -> bool {
        node != 0
            && self
                .slots
                .iter()
                .any(|slot| slot.value.is_some_and(|capability| capability.node == node))
    }

    fn allocate_node(&mut self) -> u32 {
        loop {
            let node = self.next_node;
            self.next_node = self.next_node.wrapping_add(1).max(1);
            if !self.contains_node(node) {
                return node;
            }
        }
    }

    fn observe_node(&mut self, node: u32) {
        if self.next_node <= node {
            self.next_node = node.wrapping_add(1).max(1);
        }
    }

    fn slot(&self, handle: CapHandle) -> Result<&Slot, Status> {
        let index = self.validate_index(handle)?;
        Ok(&self.slots[index])
    }

    fn validate_index(&self, handle: CapHandle) -> Result<usize, Status> {
        let index = handle.slot();
        if handle == CapHandle::INVALID
            || index >= MAX_CAPABILITIES
            || self.slots[index].generation != handle.generation()
        {
            return Err(Status::BadCapability);
        }
        Ok(index)
    }

    fn clear_slot(&mut self, index: usize) {
        let slot = &mut self.slots[index];
        slot.value = None;
        slot.generation = slot.generation.wrapping_add(1).max(1);
    }
}
