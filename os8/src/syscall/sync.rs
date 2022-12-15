use crate::sync::{Condvar, Mutex, MutexBlocking, MutexSpin, Semaphore};
use crate::syscall::sys_gettid;
use crate::task::{block_current_and_run_next, current_process, current_task};
use crate::timer::{add_timer, get_time_ms};
use alloc::sync::Arc;

pub fn sys_sleep(ms: usize) -> isize {
    let expire_ms = get_time_ms() + ms;
    let task = current_task().unwrap();
    add_timer(expire_ms, task);
    block_current_and_run_next();
    0
}

// LAB5 HINT: you might need to maintain data structures used for deadlock detection
// during sys_mutex_* and sys_semaphore_* syscalls
pub fn sys_mutex_create(blocking: bool) -> isize {
    let process = current_process();
    let mutex: Option<Arc<dyn Mutex>> = if !blocking {
        Some(Arc::new(MutexSpin::new()))
    } else {
        Some(Arc::new(MutexBlocking::new()))
    };
    let mut process_inner = process.inner_exclusive_access();
    if let Some(id) = process_inner
        .mutex_list
        .iter()
        .enumerate()
        .find(|(_, item)| item.is_none())
        .map(|(id, _)| id)
    {
        process_inner.mutex_list[id] = mutex;

        // 将新建的锁记录下来，用于之后初始化新的thread的need矩阵
        process_inner.dead_lock_detect_block.available_mutex[id] = 1;
        // 为当前所有thread初始化need和allocation矩阵
        for index in 0..process_inner.tasks.len() {
            process_inner.dead_lock_detect_block.need_mutex[index][id] = 0;
            process_inner.dead_lock_detect_block.allocation_mutex[index][id] = 0;
        }

        id as isize
    } else {
        process_inner.mutex_list.push(mutex);

        // 将新建的信号量res_count记录下来，用于之后初始化新的thread的need矩阵
        process_inner.dead_lock_detect_block.available_mutex.push(1);
        // 为当前所有thread初始化need和allocation矩阵
        for index in 0..process_inner.tasks.len() {
            process_inner.dead_lock_detect_block.need_mutex[index].push(0);
            process_inner.dead_lock_detect_block.allocation_mutex[index].push(0);
        }

        process_inner.mutex_list.len() as isize - 1
    }
}

// LAB5 HINT: Return -0xDEAD if deadlock is detected
pub fn sys_mutex_lock(mutex_id: usize) -> isize {
    let process = current_process();
    let mut process_inner = process.inner_exclusive_access();
    let mutex = Arc::clone(process_inner.mutex_list[mutex_id].as_ref().unwrap());
    let tid = sys_gettid() as usize;

    process_inner.dead_lock_detect_block.need_mutex[tid][mutex_id] += 1;

    let result = process_inner.dead_lock_detect_mutex();
    drop(process_inner);
    if result == 0 {
        mutex.lock();
        // 更新allocation matrix
        let mut process_inner = process.inner_exclusive_access();
        let dead_lock_detect_block = &mut process_inner.dead_lock_detect_block;
        dead_lock_detect_block.allocation_mutex[tid][mutex_id] += 1;
        dead_lock_detect_block.need_mutex[tid][mutex_id] -= 1;
        dead_lock_detect_block.available_mutex[mutex_id] -= 1;
    }
    result
}

pub fn sys_mutex_unlock(mutex_id: usize) -> isize {
    let process = current_process();
    let process_inner = process.inner_exclusive_access();
    let mutex = Arc::clone(process_inner.mutex_list[mutex_id].as_ref().unwrap());
    drop(process_inner);
    mutex.unlock();

    // 更新allocation matrix
    let mut process_inner = process.inner_exclusive_access();
    let tid = sys_gettid() as usize;
    let dead_lock_detect_block = &mut process_inner.dead_lock_detect_block;
    dead_lock_detect_block.allocation_mutex[tid][mutex_id] -= 1;
    dead_lock_detect_block.available_mutex[mutex_id] += 1;

    0
}

pub fn sys_semaphore_create(res_count: usize) -> isize {
    let process = current_process();
    let mut process_inner = process.inner_exclusive_access();
    let id = if let Some(id) = process_inner
        .semaphore_list
        .iter()
        .enumerate()
        .find(|(_, item)| item.is_none())
        .map(|(id, _)| id)
    {
        process_inner.semaphore_list[id] = Some(Arc::new(Semaphore::new(res_count)));

        // 将新建的信号量res_count记录下来，用于之后初始化新的thread的need矩阵
        process_inner.dead_lock_detect_block.available_semaphore[id] = res_count;
        // 为当前所有thread初始化need和allocation矩阵
        for index in 0..process_inner.tasks.len() {
            process_inner.dead_lock_detect_block.need_semaphore[index][id] = 0;
            process_inner.dead_lock_detect_block.allocation_semaphore[index][id] = 0;
        }
        id
    } else {
        process_inner
            .semaphore_list
            .push(Some(Arc::new(Semaphore::new(res_count))));

        // 将新建的信号量res_count记录下来，用于之后初始化新的thread的need矩阵
        process_inner
            .dead_lock_detect_block
            .available_semaphore
            .push(res_count);
        // 为当前所有thread初始化need和allocation矩阵
        for index in 0..process_inner.tasks.len() {
            process_inner.dead_lock_detect_block.need_semaphore[index].push(0);
            process_inner.dead_lock_detect_block.allocation_semaphore[index].push(0);
        }
        process_inner.semaphore_list.len() - 1
    };
    id as isize
}

pub fn sys_semaphore_up(sem_id: usize) -> isize {
    let process = current_process();
    let process_inner = process.inner_exclusive_access();
    let sem = Arc::clone(process_inner.semaphore_list[sem_id].as_ref().unwrap());

    drop(process_inner);
    sem.up();
    // 更新allocation matrix
    let mut process_inner = process.inner_exclusive_access();
    let tid = sys_gettid() as usize;
    let dead_lock_detect_block = &mut process_inner.dead_lock_detect_block;
    dead_lock_detect_block.allocation_semaphore[tid][sem_id] -= 1;
    dead_lock_detect_block.available_semaphore[sem_id] += 1;
    0
}

// LAB5 HINT: Return -0xDEAD if deadlock is detected
pub fn sys_semaphore_down(sem_id: usize) -> isize {
    let process = current_process();
    let mut process_inner = process.inner_exclusive_access();
    let sem = Arc::clone(process_inner.semaphore_list[sem_id].as_ref().unwrap());
    let tid = sys_gettid() as usize;

    process_inner.dead_lock_detect_block.need_semaphore[tid][sem_id] += 1;
    let result = process_inner.dead_lock_detect_semaphore();

    drop(process_inner);
    if result == 0 {
        sem.down();

        // 更新allocation matrix
        let mut process_inner = process.inner_exclusive_access();
        let dead_lock_detect_block = &mut process_inner.dead_lock_detect_block;
        dead_lock_detect_block.allocation_semaphore[tid][sem_id] += 1;
        dead_lock_detect_block.need_semaphore[tid][sem_id] -= 1;
        dead_lock_detect_block.available_semaphore[sem_id] -= 1;
    }

    result
}

pub fn sys_condvar_create(_arg: usize) -> isize {
    let process = current_process();
    let mut process_inner = process.inner_exclusive_access();
    let id = if let Some(id) = process_inner
        .condvar_list
        .iter()
        .enumerate()
        .find(|(_, item)| item.is_none())
        .map(|(id, _)| id)
    {
        process_inner.condvar_list[id] = Some(Arc::new(Condvar::new()));
        id
    } else {
        process_inner
            .condvar_list
            .push(Some(Arc::new(Condvar::new())));
        process_inner.condvar_list.len() - 1
    };
    id as isize
}

pub fn sys_condvar_signal(condvar_id: usize) -> isize {
    let process = current_process();
    let process_inner = process.inner_exclusive_access();
    let condvar = Arc::clone(process_inner.condvar_list[condvar_id].as_ref().unwrap());
    drop(process_inner);
    condvar.signal();
    0
}

pub fn sys_condvar_wait(condvar_id: usize, mutex_id: usize) -> isize {
    let process = current_process();
    let process_inner = process.inner_exclusive_access();
    let condvar = Arc::clone(process_inner.condvar_list[condvar_id].as_ref().unwrap());
    let mutex = Arc::clone(process_inner.mutex_list[mutex_id].as_ref().unwrap());
    drop(process_inner);
    condvar.wait(mutex);
    0
}

// LAB5 YOUR JOB: Implement deadlock detection, but might not all in this syscall
pub fn sys_enable_deadlock_detect(enabled: usize) -> isize {
    let process = current_process();
    let mut process_inner = process.inner_exclusive_access();
    if enabled == 1 {
        process_inner.dead_lock_detect_block.enable_dead_detect = true;
        0
    } else if enabled == 0 {
        process_inner.dead_lock_detect_block.enable_dead_detect = false;
        0
    } else {
        -1
    }
}
