#!/usr/bin/env bash
# Two private LANs, two stateful SNATs (no port forwards) and an independent
# TLS relay / QUIC address discovery server. No link in the host namespace,
# default route, public DNS/relay, host firewall change or privileged test role.
set -euo pipefail
umask 077
usage() {
    cat <<'HELP'
Usage: test_wan_nat.sh [--lose-direct-interface] [--redundant-media] [--private-stun] [--short-path-timeout] [--require-voice-deadline]

Test relay → direct UDP across two NATs → relay on the same pinned QUIC connection.
By default the routers silently blackhole the direct route. Success verifies
migration and delivery after fallback, not uninterrupted voice during the cut.

  --lose-direct-interface    Instead remove sender A's direct interface. Its private
                             TLS relay is already connected on a separate interface.
                             Only A is notified; verify the original standby survives.
  --short-path-timeout       Use test-only 250 ms path keepalive and 1 s idle timeout.
  --redundant-media          Both pinned peers explicitly duplicate PCM to a separate
                             private HTTPS relay. Reliable traffic stays on the primary.
  --private-stun             Test the production scoped constructor: paired private
                             Binding on the QUIC socket, then direct → relay.
  --require-voice-deadline   Also require every cut audio attempt to reach peer decode
                             in <5 ms one-way. Missing or late frames fail the run.
  -h, --help                 Print this help without building or creating namespaces.

Run as an unprivileged Linux build user with passwordless sudo for the namespace
fixture. The printed evidence directory retains logs; ephemeral keys and fixture
namespaces are removed on exit.
HELP
}
strict=0
media=0
discovery=qad
profile=default
cut_mode=blackhole
for arg in "$@"; do
    case "$arg" in
        --require-voice-deadline) strict=1 ;;
        --short-path-timeout) profile=short ;;
        --redundant-media) media=1 ;;
        --private-stun) discovery=stun ;;
        --lose-direct-interface) cut_mode=interface-loss ;;
        -h|--help) usage; exit 0 ;;
        *) echo "unknown option: $arg" >&2; usage >&2; exit 2 ;;
    esac
done
if [[ $discovery == stun && $profile != default ]]; then
    echo "the production scoped constructor uses the default path timers" >&2; exit 2
fi
[[ $(id -u) != 0 ]] || { echo "run as the unprivileged build user" >&2; exit 2; }
cd "$(dirname "$0")/../engine/core-rs"
stun_helper="$PWD/../../scripts/wan_private_stun.py"
report="$PWD/../../scripts/wan_cut_report.py"
scratch="$PWD/target/wan-nat-$$-$RANDOM"
mkdir -p target
mkdir "$scratch"
echo "WAN evidence: $scratch"
names=() jobs=()
cleanup() {
    local rc=$? cleanup_error=0 pid ns
    trap - EXIT
    trap '' HUP INT TERM
    if (( rc != 0 && ${#jobs[@]} )); then touch "$scratch/abort" || cleanup_error=1; fi
    # All roles have an external wall-clock limit, including on setup failure.
    for pid in "${jobs[@]}"; do wait "$pid" || cleanup_error=1; done
    for ns in "${names[@]}"; do sudo -n ip netns del "$ns" || cleanup_error=1; done
    rm -rf -- "$scratch/keys" || cleanup_error=1
    if (( rc == 0 && cleanup_error )); then rc=1; fi
    if (( rc != 0 )); then tail -n 8 "$scratch"/*.log >&2 || :; fi
    exit "$rc"
}
trap cleanup EXIT
# Defer a signal across creation and registration so teardown owns the resource.
registering=0
interrupted=0
on_signal() {
    interrupted=1
    (( registering )) || exit 1
}
trap on_signal HUP INT TERM
export TMPDIR="$scratch"
timeout 20m cargo test --locked --jobs 2 -p eon-mesh --lib --no-run --message-format=json >"$scratch/build.jsonl"
exe=$(python3 -c 'import json,sys; print(next(m["executable"] for m in map(json.loads,sys.stdin) if m.get("executable") and m["target"]["name"] == "eon_mesh"))' <"$scratch/build.jsonl")
cd "$scratch"
mkdir keys
for side in a b; do
    if [[ $discovery == stun ]]; then openssl rand -hex 32 >"keys/stun-$side.psk"; fi
    openssl genpkey -algorithm Ed25519 -out "keys/$side.pem"
    openssl pkey -in "keys/$side.pem" -outform DER | tail -c 32 | xxd -p -c 32 >"keys/$side.key"
    openssl pkey -in "keys/$side.pem" -pubout -outform DER | tail -c 32 | xxd -p -c 32 >"keys/$side.pub"
done
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -days 1 -subj /CN=eon-wan-test-ca \
    -keyout keys/ca.key -out keys/ca.pem -addext basicConstraints=critical,CA:TRUE 2>tls.log
openssl req -new -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -subj /CN=relay \
    -keyout keys/leaf.key -out keys/leaf.csr 2>>tls.log
printf 'subjectAltName=IP:10.77.0.1\nextendedKeyUsage=serverAuth\nbasicConstraints=CA:FALSE\n' >keys/ext
openssl x509 -req -in keys/leaf.csr -CA keys/ca.pem -CAkey keys/ca.key -CAcreateserial -days 1 \
    -extfile keys/ext -outform DER -out keys/leaf.der 2>>tls.log
openssl x509 -in keys/ca.pem -outform DER -out keys/ca.der
openssl pkcs8 -topk8 -nocrypt -in keys/leaf.key -outform DER -out keys/leaf.pk8

prefix="eon-wan-$$-$RANDOM"
a="$prefix-a" b="$prefix-b" na="$prefix-na" nb="$prefix-nb" relay="$prefix-relay"
ipn() { sudo -n ip -n "$@"; }
inside() { sudo -n ip netns exec "$@"; }
for ns in "$a" "$b" "$na" "$nb" "$relay"; do
    registering=1
    sudo -n ip netns add "$ns"
    names+=("$ns")
    registering=0
    (( !interrupted )) || exit 1
    ipn "$ns" link set lo up
done
ipn "$relay" link add backplane type bridge
ipn "$relay" addr add 10.77.0.1/29 dev backplane
ipn "$relay" link set backplane up
attach() {
    local ep=$1 router=$2 lan=$3 outer=$4 other=$5
    ipn "$ep" link add wan0 type veth peer name inner netns "$router"
    ipn "$router" link add outer type veth peer name "p$lan" netns "$relay"
    ipn "$relay" link set "p$lan" master backplane
    ipn "$relay" link set "p$lan" up
    ipn "$router" addr add "192.168.$lan.1/30" dev inner
    ipn "$router" addr add "10.77.0.$outer/29" dev outer
    ipn "$router" link set inner up
    ipn "$router" link set outer up
    inside "$router" sysctl -qw net.ipv4.ip_forward=1
    inside "$router" iptables -P FORWARD DROP
    # Drop an unsolicited first punch locally: don't generate an ICMP error or
    # confirm an inbound conntrack entry before the other client has punched.
    inside "$router" iptables -A INPUT -i outer -p udp --dport 41770 -j DROP
    inside "$router" iptables -A FORWARD -p udp -d "10.77.0.$other" --dport 41770
    inside "$router" iptables -A FORWARD -p udp -s "10.77.0.$other" --sport 41770
    inside "$router" iptables -A FORWARD -i inner -o outer -s "192.168.$lan.2" -d 10.77.0.0/29 -j ACCEPT
    inside "$router" iptables -A FORWARD -i outer -o inner -m conntrack --ctstate ESTABLISHED,RELATED -j ACCEPT
    if [[ $discovery == stun ]]; then
        # Count, do not drop. The client must reject before any UDP send,
        # regardless of whether the kernel could eventually deliver it.
        inside "$ep" iptables -A OUTPUT -p udp -d "10.77.0.$other" --dport 41999
        # Translate an ephemeral source port. The clients only know the pool;
        # they never read a mapped address from a marker or command line.
        inside "$router" iptables -t nat -A POSTROUTING -p udp --sport 41769 -o outer -s "192.168.$lan.2" -d 10.77.0.1 \
            -j SNAT --to-source "10.77.0.$outer:41769"
        inside "$router" iptables -t nat -A POSTROUTING -p udp -o outer -s "192.168.$lan.2" -d 10.77.0.0/29 \
            -j SNAT --to-source "10.77.0.$outer:41770"
    fi
    inside "$router" iptables -t nat -A POSTROUTING -o outer -s "192.168.$lan.2" -d 10.77.0.0/29 \
        -j SNAT --to-source "10.77.0.$outer"
    ipn "$ep" addr add "192.168.$lan.2/30" dev wan0
    ipn "$ep" link set wan0 up
    for ip in 1 "$other"; do ipn "$ep" route add "10.77.0.$ip/32" via "192.168.$lan.1"; done
    # Initially relay-only. Removing this fixture's own impairment below
    # cannot connect to anything outside the disposable backplane.
    ipn "$router" route add blackhole "10.77.0.$other/32"
}
attach "$a" "$na" 1 2 3
attach "$b" "$nb" 2 3 2
if [[ $cut_mode == interface-loss ]]; then
    # A's approved TLS origin has a separate last mile. UDP (including QAD)
    # still uses wan0 and both NATs; there is no peer route on relay0.
    ipn "$a" link add relay0 type veth peer name standby netns "$relay"
    ipn "$a" addr add 172.30.84.2/30 dev relay0
    ipn "$relay" addr add 172.30.84.1/30 dev standby
    ipn "$a" link set relay0 up
    ipn "$relay" link set standby up
    inside "$a" iptables -A OUTPUT -o relay0 -d 10.77.0.1 -p tcp --dport 8443 -j ACCEPT
    if (( media )); then
        inside "$a" iptables -A OUTPUT -o relay0 -d 10.77.0.1 -p tcp --dport 8444 -j ACCEPT
    fi
    inside "$a" iptables -A OUTPUT -o relay0 -j DROP
    inside "$a" ip6tables -A OUTPUT -o relay0 -j DROP
    ipn "$a" route add table 84 10.77.0.1/32 via 172.30.84.1 dev relay0
    ipn "$a" rule add pref 84 to 10.77.0.1/32 ipproto tcp dport 8443 lookup 84
    if (( media )); then
        ipn "$a" rule add pref 85 to 10.77.0.1/32 ipproto tcp dport 8444 lookup 84
    fi
fi
for ns in "${names[@]}"; do
    echo "$ns"
    ipn "$ns" -4 route
    [[ -z $(ipn "$ns" route show default) && -z $(ipn "$ns" -6 route show default) ]]
done >routes.log

parent=$(stat -Lc %i /proc/self/ns/net)
user=$(id -un)
role() {
    local inode
    inode=$(stat -Lc %i "/run/netns/$1")
    registering=1
    inside "$1" sudo -n -u "$user" env EON_WAN_ROLE="$2" EON_WAN_PARENT_NS="$parent" EON_WAN_NETNS="$inode" \
        EON_WAN_PATH_PROFILE="$profile" EON_WAN_CUT="$cut_mode" EON_WAN_MEDIA="$media" EON_WAN_DISCOVERY="$discovery" \
        timeout --kill-after=2s 80s "$exe" \
        --ignored --exact wan::netns::iroh_and_qos_tests_nat_path_change --nocapture \
        >"$2.log" 2>&1 &
    jobs+=("$!")
    registering=0
    (( !interrupted )) || exit 1
}
await_marker() {
    local i pid
    for ((i=0; i<500; i++)); do
        [[ ! -e $1 ]] || return 0
        for pid in "${jobs[@]}"; do kill -0 "$pid" 2>/dev/null || return 1; done
        sleep .1
    done
    echo "timed out waiting for $1" >&2
    return 1
}
if [[ $discovery == stun ]]; then
    stun() {
        inside "$1" sudo -n -u "$user" env EON_WAN_PARENT_NS="$parent" \
            EON_WAN_NETNS="$(stat -Lc %i "/run/netns/$1")" \
            timeout --kill-after=2s 65s python3 -I "$stun_helper" "$2"
    }
    registering=1
    stun "$relay" server >stun.log 2>&1 &
    jobs+=("$!")
    registering=0
    (( !interrupted )) || exit 1
    await_marker stun-ready
fi
role "$relay" relay
await_marker ready
role "$b" b
role "$a" a
if [[ $discovery == stun ]]; then
    # Distinguish this direct-first migration from the QAD relay-first test.
    # Snapshot each live kernel socket *after* Binding and before publication.
    await_marker stun-observed
    udp_cookie() { inside "$1" ss -HOaune "src $2" | awk '{for(i=1;i<=NF;i++) if ($i ~ /^sk:/) print $i}'; }
    a_cookie=$(udp_cookie "$a" 192.168.1.2) b_cookie=$(udp_cookie "$b" 192.168.2.2)
    [[ $a_cookie =~ ^sk:[[:xdigit:]]+$ && $b_cookie =~ ^sk:[[:xdigit:]]+$ ]]
else
    await_marker bootstrap-proven-a
    await_marker bootstrap-proven-b
fi
touch open-start
ipn "$na" route del blackhole 10.77.0.3/32
ipn "$nb" route del blackhole 10.77.0.2/32
touch route-open
if [[ $discovery == stun ]]; then
    touch stun-reply
    wait "${jobs[0]}"
    jobs=("${jobs[@]:1}")
    cat stun.log
fi
direct=0
await_marker direct-proven && direct=1
for router in "$na" "$nb"; do
    inside "$router" iptables -nvxL
    inside "$router" iptables -t nat -nvxL
done >nat.log
(( direct ))
if [[ $discovery == stun ]]; then
    [[ $(udp_cookie "$a" 192.168.1.2) == "$a_cookie" && $(udp_cookie "$b" 192.168.2.2) == "$b_cookie" ]]
    echo "Binding → direct QUIC retained UDP sockets a=$a_cookie b=$b_cookie" >>routes.log
    for ep in "$a" "$b"; do
        count=$(inside "$ep" iptables -nvxL OUTPUT | awk '/dpt:41999/{print $1}')
        [[ $count == 0 ]] || { echo "unexpected packets to rejected candidate: $count" >&2; exit 1; }
        echo "$ep rejected signed candidate before send: packets=$count" >>routes.log
    done
fi
standby_cookie() {
    inside "$a" ss -HOetn state established src 172.30.84.2 dst "10.77.0.1:$1" |
        awk '{for (i=1; i<=NF; i++) if ($i ~ /^sk:/) print $i}'
}
if [[ $cut_mode == interface-loss ]]; then
    standby_before=$(standby_cookie 8443)
    [[ $standby_before =~ ^sk:[[:xdigit:]]+$ ]] || { echo "expected one independent TLS relay socket" >&2; exit 1; }
    if (( media )); then
        media_before=$(standby_cookie 8444)
        [[ $media_before =~ ^sk:[[:xdigit:]]+$ && $media_before != "$standby_before" ]]
    fi
fi
python3 -c 'from pathlib import Path; import time; Path("cut-start").write_text(str(time.monotonic_ns()))'
if [[ $cut_mode == interface-loss ]]; then
    ipn "$a" link del wan0
else
    # Silent impairment at the routers. No notification on either endpoint.
    ipn "$na" route add blackhole 10.77.0.3/32
    ipn "$nb" route add blackhole 10.77.0.2/32
fi
touch cut
python3 -I -c 'from pathlib import Path; import time; Path("cut-installed").write_text(str(time.monotonic_ns()))'
if [[ $cut_mode == interface-loss ]]; then
    await_marker counter-ready
    [[ $(standby_cookie 8443) == "$standby_before" ]] || { echo "original TLS standby did not survive" >&2; exit 1; }
    echo "relay0 retained established relay socket $standby_before across wan0 removal" >>routes.log
    if (( media )); then
        [[ $(standby_cookie 8444) == "$media_before" ]] || { echo "original media TLS socket lost" >&2; exit 1; }
        echo "relay0 retained established media socket $media_before across wan0 removal" >>routes.log
    fi
fi
for pid in "${jobs[@]}"; do wait "$pid"; done
jobs=()
grep -hE '^(QAD|STUN|witness|stream|relay )' a.log b.log relay.log
python3 -I "$report" "$strict" "$profile" "$cut_mode" "$media" "$discovery" | tee summary.log
