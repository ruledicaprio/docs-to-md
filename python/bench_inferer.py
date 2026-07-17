"""Minimal concurrency benchmark for the inferer's `ExtractStream` RPC.

Fires N concurrent requests at an in-process mock-mode server (same harness
style as `smoke_test.py`) and reports per-request latency plus, for a few
candidate `MLIS_MAX_QUEUE_DEPTH` values, how many requests would have been
rejected had `mlis-serve`'s queue-depth cap been in front of them. Mock mode
skips the model entirely (and its serialization lock), so this measures the
gRPC/ThreadPoolExecutor layer's concurrency overhead and gives real numbers
to size `MLIS_MAX_QUEUE_DEPTH` against — it does not benchmark model
throughput or the Rust-side single-GPU semaphore (see the mlis-serve live
check in the roadmap notes for that).

Run from `python/`: `python bench_inferer.py [--requests N] [--concurrency N]`.
"""

import argparse
import os
import time
from concurrent.futures import ThreadPoolExecutor

os.environ.setdefault("MLIS_INFERER_MOCK", "1")

import grpc  # noqa: E402

from inferer import inferer_pb2 as pb  # noqa: E402
from inferer import inferer_pb2_grpc as pb_grpc  # noqa: E402
from inferer.server import build_server  # noqa: E402

CANDIDATE_CAPS = (1, 2, 4, 8)


def _one_request(stub) -> tuple[float, float]:
    start = time.monotonic()
    for chunk in stub.ExtractStream(pb.ExtractRequest(markdown="P<UTO passport specimen")):
        if chunk.done:
            break
    return start, time.monotonic()


def _percentile(sorted_values: list[float], pct: float) -> float:
    if not sorted_values:
        return 0.0
    idx = min(len(sorted_values) - 1, int(len(sorted_values) * pct))
    return sorted_values[idx]


def _rejections_at_cap(spans: list[tuple[float, float]], cap: int) -> int:
    """How many requests would see `cap` others already in flight at their
    own start time (i.e. would be rejected by a queue-depth gate of `cap`)."""
    rejected = 0
    for start, _ in spans:
        in_flight = sum(1 for s, e in spans if s < start < e)
        if in_flight >= cap:
            rejected += 1
    return rejected


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--requests", type=int, default=50)
    parser.add_argument("--concurrency", type=int, default=8)
    args = parser.parse_args()

    bind = "127.0.0.1:50597"
    server = build_server(bind)
    server.start()
    try:
        with grpc.insecure_channel(bind) as chan:
            grpc.channel_ready_future(chan).result(timeout=5)
            stub = pb_grpc.InfererStub(chan)

            with ThreadPoolExecutor(max_workers=args.concurrency) as pool:
                spans = list(pool.map(lambda _: _one_request(stub), range(args.requests)))

        latencies = sorted(end - start for start, end in spans)
        print(f"requests={args.requests} concurrency={args.concurrency}")
        print(
            f"latency (s): min={latencies[0]:.4f} "
            f"p50={_percentile(latencies, 0.50):.4f} "
            f"p95={_percentile(latencies, 0.95):.4f} "
            f"max={latencies[-1]:.4f}"
        )
        for cap in CANDIDATE_CAPS:
            rejected = _rejections_at_cap(spans, cap)
            print(f"MLIS_MAX_QUEUE_DEPTH={cap}: {rejected}/{args.requests} would be rejected")
    finally:
        server.stop(grace=0)


if __name__ == "__main__":
    main()
