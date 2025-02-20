use std::collections::{HashMap, VecDeque};
use std::str;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::thread;
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::disk_store::interface::*;
use crate::ingest::colgen::GenTable;
use crate::ingest::input_column::InputColumn;
use crate::ingest::raw_val::RawVal;
use crate::locustdb::Options;
use crate::mem_store::partition::Partition;
use crate::mem_store::table::*;
use crate::mem_store::*;
use crate::scheduler::disk_read_scheduler::DiskReadScheduler;
use crate::scheduler::*;

pub struct InnerLocustDB {
    tables: RwLock<HashMap<String, Table>>,
    lru: Lru,
    pub storage: Arc<dyn DiskStore>,
    disk_read_scheduler: Arc<DiskReadScheduler>,

    opts: Options,

    next_partition_id: AtomicUsize,
    running: AtomicBool,
    idle_queue: Condvar,
    task_queue: Mutex<VecDeque<Arc<dyn Task>>>,
}

impl InnerLocustDB {
    pub fn new(storage: Arc<dyn DiskStore>, opts: &Options) -> InnerLocustDB {
        let lru = Lru::default();
        let existing_tables = Table::load_table_metadata(1 << 20, storage.as_ref(), &lru);
        let max_pid = existing_tables.values().map(|t| t.max_partition_id())
            .max()
            .unwrap_or(0);
        let disk_read_scheduler = Arc::new(DiskReadScheduler::new(
            storage.clone(),
            lru.clone(),
            opts.read_threads,
            !opts.mem_lz4,
        ));

        InnerLocustDB {
            tables: RwLock::new(existing_tables),
            lru,
            storage,
            disk_read_scheduler,
            running: AtomicBool::new(true),

            opts: opts.clone(),

            next_partition_id: AtomicUsize::new(max_pid as usize + 1),
            idle_queue: Condvar::new(),
            task_queue: Mutex::new(VecDeque::new()),
        }
    }

    pub fn start_worker_threads(locustdb: &Arc<InnerLocustDB>) {
        for _ in 0..locustdb.opts.threads {
            let cloned = locustdb.clone();
            thread::spawn(move || InnerLocustDB::worker_loop(cloned));
        }
        let cloned = locustdb.clone();
        thread::spawn(move || InnerLocustDB::enforce_mem_limit(&cloned));
    }

    pub fn snapshot(&self, table: &str) -> Option<Vec<Arc<Partition>>> {
        let tables = self.tables.read().unwrap();
        tables.get(table).map(|t| t.snapshot())
    }

    pub fn full_snapshot(&self) -> Vec<Vec<Arc<Partition>>> {
        let tables = self.tables.read().unwrap();
        tables.values().map(|t| t.snapshot()).collect()
    }

    pub fn stop(&self) {
        // Acquire task_queue_guard to make sure that there are no threads that have checked self.running but not waited on idle_queue yet.
        info!("Stopping database...");
        let _guard = self.task_queue.lock();
        self.running.store(false, Ordering::SeqCst);
        self.idle_queue.notify_all();
    }

    fn worker_loop(locustdb: Arc<InnerLocustDB>) {
        while locustdb.running.load(Ordering::SeqCst) {
            if let Some(task) = InnerLocustDB::await_task(&locustdb) {
                task.execute();
            }
        }
        drop(locustdb) // Make clippy happy
    }

    fn await_task(ldb: &Arc<InnerLocustDB>) -> Option<Arc<dyn Task>> {
        let mut task_queue = ldb.task_queue.lock().unwrap();
        while task_queue.is_empty() {
            if !ldb.running.load(Ordering::SeqCst) {
                return None;
            }
            task_queue = ldb.idle_queue.wait(task_queue).unwrap();
        }
        while let Some(task) = task_queue.pop_front() {
            if task.completed() {
                continue;
            }
            if task.multithreaded() {
                task_queue.push_front(task.clone());
            }
            if !task_queue.is_empty() {
                ldb.idle_queue.notify_one();
            }
            return Some(task);
        }
        None
    }

    pub fn schedule<T: Task + 'static>(&self, task: T) {
        // This function may be entered by event loop thread so it's important it always returns quickly.
        // Since the task queue locks are never held for long, we should be fine.
        let mut task_queue = self.task_queue.lock().unwrap();
        task_queue.push_back(Arc::new(task));
        self.idle_queue.notify_one();
    }

    pub fn store_partition(&self, tablename: &str, partition: Vec<Arc<Column>>) {
        self.create_if_empty(tablename);
        let tables = self.tables.read().unwrap();
        let table = tables.get(tablename).unwrap();
        let pid = self.next_partition_id.fetch_add(1, Ordering::SeqCst) as u64;
        self.storage.store_partition(pid, tablename, &partition);
        let (new_partition, keys) = Partition::new(pid, partition, self.lru.clone());
        table.load_partition(new_partition);
        for key in keys {
            self.lru.put(key);
        }
    }

    pub fn ingest(&self, table: &str, row: Vec<(String, RawVal)>) {
        self.create_if_empty(table);
        let tables = self.tables.read().unwrap();
        tables.get(table).unwrap().ingest(row)
    }

    pub fn restore(&self, id: PartitionID, column: Column) {
        let column = Arc::new(column);
        for table in self.tables.read().unwrap().values() {
            table.restore(id, &column);
        }
    }

    #[allow(dead_code)]
    pub fn ingest_homogeneous(&self, table: &str, columns: HashMap<String, InputColumn>) {
        self.create_if_empty(table);
        let tables = self.tables.read().unwrap();
        tables.get(table).unwrap().ingest_homogeneous(columns)
    }

    #[allow(dead_code)]
    pub fn ingest_heterogeneous(&self, table: &str, columns: HashMap<String, Vec<RawVal>>) {
        self.create_if_empty(table);
        let tables = self.tables.read().unwrap();
        tables.get(table).unwrap().ingest_heterogeneous(columns)
    }

    pub fn drop_pending_tasks(&self) {
        let mut task_queue = self.task_queue.lock().unwrap();
        task_queue.clear();
    }

    pub fn mem_tree(&self, depth: usize) -> Vec<MemTreeTable> {
        let tables = self.tables.read().unwrap();
        tables.values().map(|table| table.mem_tree(depth)).collect()
    }

    pub fn stats(&self) -> Vec<TableStats> {
        let tables = self.tables.read().unwrap();
        tables.values().map(|table| table.stats()).collect()
    }

    pub fn gen_partition(&self, opts: &GenTable, p: u64) {
        opts.gen(self, p);
    }

    fn create_if_empty(&self, table: &str) {
        let exists = {
            let tables = self.tables.read().unwrap();
            tables.contains_key(table)
        };
        if !exists {
            {
                let mut tables = self.tables.write().unwrap();
                tables.insert(
                    table.to_string(),
                    Table::new(1 << 20, table, self.lru.clone()),
                );
            }
            self.ingest(
                "_meta_tables",
                vec![
                    (
                        "timestamp".to_string(),
                        RawVal::Int(SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64)
                    ),
                    ("name".to_string(), RawVal::Str(table.to_string())),
                ],
            );
        }
    }

    fn enforce_mem_limit(ldb: &Arc<InnerLocustDB>) {
        while ldb.running.load(Ordering::SeqCst) {
            let mut mem_usage_bytes: usize = {
                let tables = ldb.tables.read().unwrap();
                tables
                    .values()
                    .map(|table| table.heap_size_of_children())
                    .sum()
            };
            if mem_usage_bytes > ldb.opts.mem_size_limit_tables {
                info!("Evicting. mem_usage_bytes = {}", mem_usage_bytes);
                while mem_usage_bytes > ldb.opts.mem_size_limit_tables {
                    match ldb.lru.evict() {
                        Some(victim) => {
                            let tables = ldb.tables.read().unwrap();
                            for t in tables.values() {
                                mem_usage_bytes -= t.evict(&victim);
                            }
                        }
                        None => {
                            if ldb.opts.mem_size_limit_tables > 0 {
                                warn!(
                                    "Table memory usage is {} but failed to find column to evict!",
                                    mem_usage_bytes
                                );
                            }
                            break;
                        }
                    }
                }
                info!("mem_usage_bytes = {}", mem_usage_bytes);
            }
            thread::sleep(Duration::from_millis(1000));
        }
    }

    pub fn max_partition_id(&self) -> u64 {
        self.next_partition_id.load(Ordering::SeqCst) as u64
    }

    pub fn opts(&self) -> &Options {
        &self.opts
    }

    pub fn disk_read_scheduler(&self) -> &Arc<DiskReadScheduler> {
        &self.disk_read_scheduler
    }
}

impl Drop for InnerLocustDB {
    fn drop(&mut self) {
        info!("Stopped");
    }
}
