use crate::observe::Observe;
use eth2::lighthouse::{ProcessHealth, SystemHealth};
use metrics::*;
use std::sync::LazyLock;

pub static PROCESS_NUM_THREADS: LazyLock<Result<IntGauge>> = LazyLock::new(|| {
    try_create_int_gauge(
        "process_num_threads",
        "Number of threads used by the current process",
    )
});
pub static PROCESS_RES_MEM: LazyLock<Result<IntGauge>> = LazyLock::new(|| {
    try_create_int_gauge(
        "process_resident_memory_bytes",
        "Resident memory used by the current process",
    )
});
pub static PROCESS_VIRT_MEM: LazyLock<Result<IntGauge>> = LazyLock::new(|| {
    try_create_int_gauge(
        "process_virtual_memory_bytes",
        "Virtual memory used by the current process",
    )
});
pub static PROCESS_SHR_MEM: LazyLock<Result<IntGauge>> = LazyLock::new(|| {
    try_create_int_gauge(
        "process_shared_memory_bytes",
        "Shared memory used by the current process",
    )
});
pub static PROCESS_SECONDS: LazyLock<Result<IntGauge>> = LazyLock::new(|| {
    try_create_int_gauge(
        "process_cpu_seconds_total",
        "Total cpu time taken by the current process",
    )
});
pub static SYSTEM_VIRT_MEM_TOTAL: LazyLock<Result<IntGauge>> = LazyLock::new(|| {
    try_create_int_gauge("system_virt_mem_total_bytes", "Total system virtual memory")
});
pub static SYSTEM_VIRT_MEM_AVAILABLE: LazyLock<Result<IntGauge>> = LazyLock::new(|| {
    try_create_int_gauge(
        "system_virt_mem_available_bytes",
        "Available system virtual memory",
    )
});
pub static SYSTEM_VIRT_MEM_USED: LazyLock<Result<IntGauge>> = LazyLock::new(|| {
    try_create_int_gauge("system_virt_mem_used_bytes", "Used system virtual memory")
});
pub static SYSTEM_VIRT_MEM_FREE: LazyLock<Result<IntGauge>> = LazyLock::new(|| {
    try_create_int_gauge("system_virt_mem_free_bytes", "Free system virtual memory")
});
pub static SYSTEM_VIRT_MEM_CACHED: LazyLock<Result<IntGauge>> = LazyLock::new(|| {
    try_create_int_gauge("system_virt_mem_cached_bytes", "Used system virtual memory")
});
pub static SYSTEM_VIRT_MEM_BUFFERS: LazyLock<Result<IntGauge>> = LazyLock::new(|| {
    try_create_int_gauge("system_virt_mem_buffer_bytes", "Free system virtual memory")
});
pub static SYSTEM_VIRT_MEM_PERCENTAGE: LazyLock<Result<Gauge>> = LazyLock::new(|| {
    try_create_float_gauge(
        "system_virt_mem_percentage",
        "Percentage of used virtual memory",
    )
});
pub static SYSTEM_LOADAVG_1: LazyLock<Result<Gauge>> =
    LazyLock::new(|| try_create_float_gauge("system_loadavg_1", "Loadavg over 1 minute"));
pub static SYSTEM_LOADAVG_5: LazyLock<Result<Gauge>> =
    LazyLock::new(|| try_create_float_gauge("system_loadavg_5", "Loadavg over 5 minutes"));
pub static SYSTEM_LOADAVG_15: LazyLock<Result<Gauge>> =
    LazyLock::new(|| try_create_float_gauge("system_loadavg_15", "Loadavg over 15 minutes"));

pub static CPU_CORES: LazyLock<Result<IntGauge>> =
    LazyLock::new(|| try_create_int_gauge("cpu_cores", "Number of physical cpu cores"));
pub static CPU_THREADS: LazyLock<Result<IntGauge>> =
    LazyLock::new(|| try_create_int_gauge("cpu_threads", "Number of logical cpu cores"));

pub static CPU_SYSTEM_SECONDS_TOTAL: LazyLock<Result<IntGauge>> = LazyLock::new(|| {
    try_create_int_gauge(
        "cpu_system_seconds_total",
        "Total time spent in kernel mode",
    )
});
pub static CPU_USER_SECONDS_TOTAL: LazyLock<Result<IntGauge>> = LazyLock::new(|| {
    try_create_int_gauge("cpu_user_seconds_total", "Total time spent in user mode")
});
pub static CPU_IOWAIT_SECONDS_TOTAL: LazyLock<Result<IntGauge>> = LazyLock::new(|| {
    try_create_int_gauge(
        "cpu_iowait_seconds_total",
        "Total time spent waiting for io",
    )
});
pub static CPU_IDLE_SECONDS_TOTAL: LazyLock<Result<IntGauge>> =
    LazyLock::new(|| try_create_int_gauge("cpu_idle_seconds_total", "Total time spent idle"));

pub static DISK_BYTES_TOTAL: LazyLock<Result<IntGauge>> =
    LazyLock::new(|| try_create_int_gauge("disk_node_bytes_total", "Total capacity of disk"));

pub static DISK_BYTES_FREE: LazyLock<Result<IntGauge>> =
    LazyLock::new(|| try_create_int_gauge("disk_node_bytes_free", "Free space in disk"));

pub static DISK_READS: LazyLock<Result<IntGauge>> =
    LazyLock::new(|| try_create_int_gauge("disk_node_reads_total", "Number of disk reads"));

pub static DISK_WRITES: LazyLock<Result<IntGauge>> =
    LazyLock::new(|| try_create_int_gauge("disk_node_writes_total", "Number of disk writes"));

pub static NETWORK_BYTES_RECEIVED: LazyLock<Result<IntGauge>> = LazyLock::new(|| {
    try_create_int_gauge(
        "network_node_bytes_total_received",
        "Total bytes received over all network interfaces",
    )
});
pub static NETWORK_BYTES_SENT: LazyLock<Result<IntGauge>> = LazyLock::new(|| {
    try_create_int_gauge(
        "network_node_bytes_total_transmit",
        "Total bytes sent over all network interfaces",
    )
});

pub static BOOT_TIME: LazyLock<Result<IntGauge>> = LazyLock::new(|| {
    try_create_int_gauge(
        "misc_node_boot_ts_seconds",
        "Boot time as unix epoch timestamp",
    )
});

pub fn scrape_health_metrics() {
    scrape_process_health_metrics();
    scrape_system_health_metrics();
}

pub fn scrape_process_health_metrics() {
    // This will silently fail if we are unable to observe the health. This is desired behaviour
    // since we don't support `Health` for all platforms.
    if let Ok(health) = ProcessHealth::observe() {
        set_gauge(&PROCESS_NUM_THREADS, health.pid_num_threads);
        set_gauge(&PROCESS_RES_MEM, health.pid_mem_resident_set_size as i64);
        set_gauge(&PROCESS_VIRT_MEM, health.pid_mem_virtual_memory_size as i64);
        set_gauge(&PROCESS_SHR_MEM, health.pid_mem_shared_memory_size as i64);
        set_gauge(&PROCESS_SECONDS, health.pid_process_seconds_total as i64);
    }
}

pub fn scrape_system_health_metrics() {
    // This will silently fail if we are unable to observe the health. This is desired behaviour
    // since we don't support `Health` for all platforms.
    if let Ok(health) = SystemHealth::observe() {
        set_gauge(&SYSTEM_VIRT_MEM_TOTAL, health.sys_virt_mem_total as i64);
        set_gauge(
            &SYSTEM_VIRT_MEM_AVAILABLE,
            health.sys_virt_mem_available as i64,
        );
        set_gauge(&SYSTEM_VIRT_MEM_USED, health.sys_virt_mem_used as i64);
        set_gauge(&SYSTEM_VIRT_MEM_FREE, health.sys_virt_mem_free as i64);
        set_float_gauge(
            &SYSTEM_VIRT_MEM_PERCENTAGE,
            health.sys_virt_mem_percent as f64,
        );
        set_float_gauge(&SYSTEM_LOADAVG_1, health.sys_loadavg_1);
        set_float_gauge(&SYSTEM_LOADAVG_5, health.sys_loadavg_5);
        set_float_gauge(&SYSTEM_LOADAVG_15, health.sys_loadavg_15);

        set_gauge(&CPU_CORES, health.cpu_cores as i64);
        set_gauge(&CPU_THREADS, health.cpu_threads as i64);

        set_gauge(
            &CPU_SYSTEM_SECONDS_TOTAL,
            health.system_seconds_total as i64,
        );
        set_gauge(&CPU_USER_SECONDS_TOTAL, health.user_seconds_total as i64);
        set_gauge(
            &CPU_IOWAIT_SECONDS_TOTAL,
            health.iowait_seconds_total as i64,
        );
        set_gauge(&CPU_IDLE_SECONDS_TOTAL, health.idle_seconds_total as i64);

        set_gauge(&DISK_BYTES_TOTAL, health.disk_node_bytes_total as i64);

        set_gauge(&DISK_BYTES_FREE, health.disk_node_bytes_free as i64);
        set_gauge(&DISK_READS, health.disk_node_reads_total as i64);
        set_gauge(&DISK_WRITES, health.disk_node_writes_total as i64);

        set_gauge(
            &NETWORK_BYTES_RECEIVED,
            health.network_node_bytes_total_received as i64,
        );
        set_gauge(
            &NETWORK_BYTES_SENT,
            health.network_node_bytes_total_transmit as i64,
        );
    }
}
