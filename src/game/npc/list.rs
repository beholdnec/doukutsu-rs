use std::cell::{Cell, Ref, RefCell, RefMut};
use std::ops::{Deref, DerefMut};

use crate::framework::error::{GameError, GameResult};
use crate::game::npc::NPC;

/// Maximum capacity of NPCList
const NPC_LIST_MAX_CAP: usize = 512;

pub struct NPCCell(RefCell<NPC>);

// Enforces the rules of the NPC list: All borrows must be returned before performing certain operations.
pub struct NPCAccessToken {
    // Prevent forging an NPCAccessToken outside this module
    _private: ()
}

pub trait TokenProvider {
    fn unborrow_then<'a: 'b, 'b, F, T>(&'a mut self, f: F) -> T
    where
        F: FnOnce(&'b NPCAccessToken) -> T;
}

impl TokenProvider for NPCAccessToken {
    fn unborrow_then<'a: 'b, 'b, F, T>(&'a mut self, f: F) -> T
    where
        F: FnOnce(&'b NPCAccessToken) -> T
    {
        f(self)
    }
}

pub enum BorrowedNPCRefMut<'a> {
    Borrowed { ref_mut: RefMut<'a, NPC>, token: &'a NPCAccessToken, cell: &'a NPCCell },
    Unborrowed,
}

impl Deref for BorrowedNPCRefMut<'_> {
    type Target = NPC;

    fn deref(&self) -> &Self::Target {
        match self {
            BorrowedNPCRefMut::Borrowed { ref_mut, .. } => &*ref_mut,
            _ => unreachable!()
        }
    }
}

impl DerefMut for BorrowedNPCRefMut<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        match self {
            BorrowedNPCRefMut::Borrowed { ref_mut, .. } => &mut *ref_mut,
            _ => unreachable!()
        }
    }
}

impl TokenProvider for BorrowedNPCRefMut<'_> {
    fn unborrow_then<'a: 'b, 'b, F, T>(&'a mut self, f: F) -> T
    where
        F: FnOnce(&'b NPCAccessToken) -> T
    {
        match self {
            BorrowedNPCRefMut::Borrowed { .. } => {
                let old = std::mem::replace(self, BorrowedNPCRefMut::Unborrowed);
                let (old_ref_mut, token, cell) = match old {
                    BorrowedNPCRefMut::Borrowed { ref_mut, token, cell } => (ref_mut, token, cell),
                    _ => unreachable!()
                };
                
                drop(old_ref_mut);

                let result = f(token);
            
                *self = cell.borrow_mut(token);

                result
            }
            _ => unreachable!()
        }
    }
}

impl NPCCell {
    pub fn borrow(&self, _token: &NPCAccessToken) -> Ref<'_, NPC> {
        // TODO: get rid of this function?
        self.0.borrow()
    }

    /// Borrows the NPC without an access token. The caller is responsible for preventing multiple-borrow panics.
    pub fn borrow_unmanaged(&self) -> Ref<'_, NPC> {
        self.0.borrow()
    }

    /// Mutably borrows the NPC. The access token is taken and held until the borrow is dropped.
    pub fn borrow_mut<'a>(&'a self, token: &'a NPCAccessToken) -> BorrowedNPCRefMut<'a> {
        BorrowedNPCRefMut::Borrowed {
            ref_mut: self.0.borrow_mut(),
            token,
            cell: self
        }
    }

    /// Borrows the NPC without an access token. The caller is responsible for preventing borrow panics.
    pub fn borrow_mut_unmanaged(&self) -> RefMut<'_, NPC> {
        self.0.borrow_mut()
    }
}


/// A data structure for storing an NPC list for current stage.
/// Provides multiple mutable references to NPC objects with internal sanity checks and lifetime bounds.
pub struct NPCList {
    // UnsafeCell is required because we do mutable aliasing (ik, discouraged), prevents Rust/LLVM
    // from theoretically performing some optimizations that might break the code.
    npcs: Box<[NPCCell; NPC_LIST_MAX_CAP]>,
    max_npc: Cell<u16>,
    seed: i32,
}

#[allow(dead_code)]
impl NPCList {
    pub fn new() -> (NPCList, NPCAccessToken) {
        let map = NPCList {
            npcs: Box::new(std::array::from_fn(|_| NPCCell(RefCell::new(NPC::empty())))),
            max_npc: Cell::new(0),
            seed: 0,
        };

        let token = NPCAccessToken { _private: () };

        for (idx, npc_ref) in map.npcs.iter().enumerate() {
            npc_ref.borrow_mut(&token).id = idx as u16;
        }

        (map, token)
    }

    pub fn set_rng_seed(&mut self, seed: i32) {
        self.seed = seed;
    }

    /// Inserts NPC into list in first available slot after given ID.
    pub fn spawn(&self, min_id: u16, mut npc: NPC) -> GameResult {
        let npc_len = self.npcs.len();

        if min_id as usize >= npc_len {
            return Err(GameError::InvalidValue("NPC ID is out of bounds".to_string()));
        }

        for id in min_id..(npc_len as u16) {
            let npc_ref = self.npcs.get(id as usize).unwrap();

            if !npc_ref.0.try_borrow().is_ok_and(|npc_ref| npc_ref.cond.alive()) {
                npc.id = id;

                if npc.tsc_direction == 0 {
                    npc.tsc_direction = npc.direction as u16;
                }

                npc.init_rng(self.seed);

                npc_ref.0.replace(npc);

                if self.max_npc.get() <= id {
                    self.max_npc.replace(id + 1);
                }

                return Ok(());
            }
        }

        Err(GameError::InvalidValue("No free NPC slot found!".to_string()))
    }

    /// Inserts the NPC at specified slot.
    pub fn spawn_at_slot(&self, id: u16, mut npc: NPC) -> GameResult {
        let npc_len = self.npcs.len();

        if id as usize >= npc_len {
            return Err(GameError::InvalidValue("NPC ID is out of bounds".to_string()));
        }

        npc.id = id;

        if npc.tsc_direction == 0 {
            npc.tsc_direction = npc.direction as u16;
        }

        npc.init_rng(self.seed);

        let npc_ref = self.npcs.get(id as usize).unwrap();
        npc_ref.0.replace(npc);

        if self.max_npc.get() <= id {
            self.max_npc.replace(id + 1);
        }

        Ok(())
    }

    /// Returns a reference to NPC from this list.
    pub fn get_npc(&self, id: usize) -> Option<&NPCCell> {
        self.npcs.get(id)
    }

    /// Returns an iterator that iterates over allocated (not up to it's capacity) NPC slots.
    pub fn iter(&self) -> NPCListMutableIterator<'_> {
        NPCListMutableIterator::new(self)
    }

    /// Returns an iterator over alive NPC slots.
    pub fn iter_alive<'a>(&'a self, token: &'a NPCAccessToken) -> NPCListMutableAliveIterator<'a> {
        NPCListMutableAliveIterator::new(self, token)
    }

    /// Removes all NPCs from this list and resets it's capacity.
    pub fn clear(&self, token: &NPCAccessToken) {
        for (idx, mut npc) in self.iter_alive(token).enumerate() {
            *npc = NPC::empty();
            npc.id = idx as u16;
        }

        self.max_npc.replace(0);
    }

    /// Returns current capacity of this NPC list.
    pub fn current_capacity(&self) -> u16 {
        self.max_npc.get()
    }

    /// Returns maximum capacity of this NPC list.
    pub fn max_capacity(&self) -> u16 {
        NPC_LIST_MAX_CAP as u16
    }
}

pub struct NPCListMutableIterator<'a> {
    index: u16,
    map: &'a NPCList,
}

impl<'a> NPCListMutableIterator<'a> {
    pub fn new(map: &'a NPCList) -> NPCListMutableIterator<'a> {
        NPCListMutableIterator { index: 0, map }
    }
}

impl<'a> Iterator for NPCListMutableIterator<'a> {
    type Item = &'a NPCCell;

    fn next(&mut self) -> Option<Self::Item> {
        if self.index >= self.map.max_npc.get() {
            return None;
        }

        let item = self.map.npcs.get(self.index as usize);
        self.index += 1;

        item
    }
}

pub struct NPCListMutableAliveIterator<'a> {
    index: u16,
    map: &'a NPCList,
    token: &'a NPCAccessToken,
}

impl<'a> NPCListMutableAliveIterator<'a> {
    pub fn new(map: &'a NPCList, token: &'a NPCAccessToken) -> NPCListMutableAliveIterator<'a> {
        NPCListMutableAliveIterator { index: 0, map, token }
    }
}

impl<'a> Iterator for NPCListMutableAliveIterator<'a> {
    type Item = BorrowedNPCRefMut<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.index >= self.map.max_npc.get() {
                return None;
            }

            let item = self.map.npcs.get(self.index as usize);
            self.index += 1;

            match item {
                None => {
                    return None;
                }
                Some(ref npc) if npc.borrow(self.token).cond.alive() => {
                    return Some(item?.borrow_mut(self.token));
                }
                _ => {}
            }
        }
    }
}

#[test]
pub fn test_npc_list() -> GameResult {
    impl NPC {
        fn test_tick(&mut self, _map: &NPCList) -> GameResult {
            self.action_counter += 1;

            Ok(())
        }
    }

    let mut npc = NPC::empty();
    npc.cond.set_alive(true);

    {
        let (map, token) = NPCList::new();
        let mut ctr = 20;

        map.spawn(0, npc.clone())?;
        map.spawn(2, npc.clone())?;
        map.spawn(256, npc.clone())?;

        assert_eq!(map.iter_alive(&token).count(), 3);

        for npc_ref in map.iter() {
            let mut npc_ref = npc_ref.borrow_mut(&token);

            if ctr > 0 {
                ctr -= 1;
                map.spawn(100, npc.clone())?;
                map.spawn(400, npc.clone())?;
            }

            if npc_ref.cond.alive() {
                npc_ref.test_tick(&map)?;
            }
        }

        assert_eq!(map.iter_alive(&token).count(), 43);

        for npc_ref in map.iter().skip(256) {
            let mut npc_ref = npc_ref.borrow_mut(&token);

            if npc_ref.cond.alive() {
                npc_ref.cond.set_alive(false);
            }
        }

        assert_eq!(map.iter_alive(&token).count(), 22);

        assert!(map.spawn((NPC_LIST_MAX_CAP + 1) as u16, npc.clone()).is_err());

        map.clear(&token);
        assert_eq!(map.iter_alive(&token).count(), 0);

        for i in 0..map.max_capacity() {
            map.spawn(i, npc.clone())?;
        }

        assert!(map.spawn(0, npc.clone()).is_err());
    }

    Ok(())
}
