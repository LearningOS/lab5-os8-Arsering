use crate::{
    mm::kernel_token,
    task::{add_task, current_task, TaskControlBlock},
    trap::{trap_handler, TrapContext},
};
use alloc::sync::Arc;
use alloc::vec::Vec;

use super::sync::sys_enable_deadlock_detect;

/// entry:是一个函数的地址，将它写入新建的task的tarpContext的sepc中，这样在调度到task时，trap返回后会自动执行这个函数
pub fn sys_thread_create(entry: usize, arg: usize) -> isize {
    let task = current_task().unwrap();
    let process = task.process.upgrade().unwrap(); // upgrade的结果：如果process已死，返回一个None
                                                   // create a new thread
    let new_task = Arc::new(TaskControlBlock::new(
        Arc::clone(&process),
        task.inner_exclusive_access()
            .res
            .as_ref()
            .unwrap()
            .ustack_base,
        true,
    ));
    let new_task_inner = new_task.inner_exclusive_access();
    let new_task_res = new_task_inner.res.as_ref().unwrap();
    let new_task_tid = new_task_res.tid;
    let new_task_trap_cx = new_task_inner.get_trap_cx();
    *new_task_trap_cx = TrapContext::app_init_context(
        entry,
        new_task_res.ustack_top(),
        kernel_token(),
        new_task.kernel_stack.get_top(),
        trap_handler as usize,
    );
    (*new_task_trap_cx).x[10] = arg;

    let mut process_inner = process.inner_exclusive_access();
    // add new thread to current process
    let tasks = &mut process_inner.tasks;
    while tasks.len() < new_task_tid + 1 {
        tasks.push(None);
    }

    tasks[new_task_tid] = Some(Arc::clone(&new_task));
    drop(tasks);

    // 为dead_lock_detect_block初始化: 为新建的thread更新need矩阵
    let dead_lock_detect_block = &mut process_inner.dead_lock_detect_block;
    while dead_lock_detect_block.need_semaphore.len() < new_task_tid + 1 {
        dead_lock_detect_block.need_semaphore.push(Vec::new());
        dead_lock_detect_block.allocation_semaphore.push(Vec::new());
        dead_lock_detect_block.need_mutex.push(Vec::new());
        dead_lock_detect_block.allocation_mutex.push(Vec::new());
    }
    for (id, _) in dead_lock_detect_block.available_semaphore.iter().enumerate() {
        if dead_lock_detect_block.need_semaphore[new_task_tid].get(id).is_none(){
            dead_lock_detect_block.need_semaphore[new_task_tid].push(0);
            dead_lock_detect_block.allocation_semaphore[new_task_tid].push(0);
        } else {
            dead_lock_detect_block.need_semaphore[new_task_tid][id] = 0;
            dead_lock_detect_block.allocation_semaphore[new_task_tid][id] = 0;
        }
    }

    for (id, _) in dead_lock_detect_block.available_mutex.iter().enumerate() {
        if dead_lock_detect_block.need_mutex[new_task_tid].get(id).is_none(){
            dead_lock_detect_block.need_mutex[new_task_tid].push(0);
            dead_lock_detect_block.allocation_mutex[new_task_tid].push(0);
        } else {
            dead_lock_detect_block.need_mutex[new_task_tid][id] = 0;
            dead_lock_detect_block.allocation_mutex[new_task_tid][id] = 0;
        }
    }

    // add new task to scheduler
    add_task(Arc::clone(&new_task));
    new_task_tid as isize
}

pub fn sys_gettid() -> isize {
    current_task()
        .unwrap()
        .inner_exclusive_access()
        .res
        .as_ref()
        .unwrap()
        .tid as isize
}

/// thread does not exist, return -1
/// thread has not exited yet, return -2
/// otherwise, return thread's exit code
pub fn sys_waittid(tid: usize) -> i32 {
    let task = current_task().unwrap();
    let process = task.process.upgrade().unwrap();
    let task_inner = task.inner_exclusive_access();
    let mut process_inner = process.inner_exclusive_access();
    // a thread cannot wait for itself
    // 如果是线程等自己，返回错误
    if task_inner.res.as_ref().unwrap().tid == tid {
        return -1;
    }

    // 如果找到 tid 对应的退出线程，则收集该退出线程的退出码 exit_tid ，否则返回错误
    let mut exit_code: Option<i32> = None;
    let waited_task = process_inner.tasks[tid].as_ref();
    if let Some(waited_task) = waited_task {
        if let Some(waited_exit_code) = waited_task.inner_exclusive_access().exit_code {
            exit_code = Some(waited_exit_code);
        }
    } else {
        // waited thread does not exist
        return -1;
    }

    if let Some(exit_code) = exit_code {
        // dealloc the exited thread
        process_inner.tasks[tid] = None;
        exit_code
    } else {
        // waited thread has not exited
        -2
    }
}
