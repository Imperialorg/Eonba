// Counts execve calls. Loaded by the eon-ebpf crate: the Rust side reads EXEC_COUNT[0] and compares it before and after
// running programs. Self-contained (no libbpf headers): the helper is called by its stable ABI id.
typedef unsigned int __u32;
typedef unsigned long long __u64;

#define SEC(name) __attribute__((section(name), used))
#define __uint(name, val) int(*name)[val]
#define __type(name, val) typeof(val) *name

#define BPF_MAP_TYPE_ARRAY 2

struct {
    __uint(type, BPF_MAP_TYPE_ARRAY);
    __uint(max_entries, 1);
    __type(key, __u32);
    __type(value, __u64);
} EXEC_COUNT SEC(".maps");

static void *(*bpf_map_lookup_elem)(void *map, const void *key) = (void *)1;

SEC("tracepoint/syscalls/sys_enter_execve")
int count_execve(void *ctx)
{
    __u32 key = 0;
    __u64 *value = bpf_map_lookup_elem(&EXEC_COUNT, &key);
    if (value)
        __sync_fetch_and_add(value, 1);
    return 0;
}

char LICENSE[] SEC("license") = "GPL";
