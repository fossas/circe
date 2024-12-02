# Reference: Static binary

Use `musl` with the `jemalloc` allocator to statically link libc.

Recommendations:
- Publish two binaries for Linux, one which statically links libc and one which does not.
- Do not attempt to statically link libc on Windows or macOS; their syscalls change between versions.

FFI libraries are linked depending on cargo link arguments, regardless of whether libc is linked
statically or dynamically. To ensure a fully static binary, ensure your libraries all use
the correct cargo arguments.

Native Rust libraries are always linked statically.

## Why `musl`?

Rust, as we all know, links statically!

... Except that really only applies to _crates_. When we start talking about FFI,
Rust programs _can_ link with C FFI statically, or they can do it dynamically.

And what about the core runtime, like the allocator? By default, Rust programs
dynamically link to the system `libc`, like any C program would!

```
❯ cargo build --release
❯ ldd target/release/broker
  linux-vdso.so.1 (0x00007ffc763fe000)
        libbz2.so.1.0 => /usr/lib/libbz2.so.1.0 (0x00007effa5181000)
        libgcc_s.so.1 => /usr/lib/libgcc_s.so.1 (0x00007effa515c000)
        libm.so.6 => /usr/lib/libm.so.6 (0x00007effa4313000)
        libc.so.6 => /usr/lib/libc.so.6 (0x00007effa4129000)
        /lib64/ld-linux-x86-64.so.2 => /usr/lib64/ld-linux-x86-64.so.2 (0x00007effa51a5000)
```

Usually this is fine, and even preferred. It lets system operators choose which
version of `libc` to use, and additionally reduces the size of the binary.
Unfortunately this can cause problems:

```
❯ docker run -it --rm roboxes/rhel8
❯ sudo docker cp target/release/broker 0fd8973ab69c:/root/broker
[root@0fd8973ab69c ~]# ln -s /usr/lib64/libbz2.so.1 /usr/lib64/libbz2.so.1.0
[root@0fd8973ab69c ~]# ./broker --version
./broker: /lib64/libm.so.6: version `GLIBC_2.29' not found (required by ./broker)
./broker: /lib64/libc.so.6: version `GLIBC_2.32' not found (required by ./broker)
./broker: /lib64/libc.so.6: version `GLIBC_2.29' not found (required by ./broker)
./broker: /lib64/libc.so.6: version `GLIBC_2.33' not found (required by ./broker)
./broker: /lib64/libc.so.6: version `GLIBC_2.34' not found (required by ./broker)
[root@0fd8973ab69c ~]# ldd --version
ldd (GNU libc) 2.28
```

If we build against `musl` instead, we can avoid this issue:

```
❯ cross build --target=x86_64-unknown-linux-musl --features jemalloc --release
❯ ldd target/x86_64-unknown-linux-musl/release/broker
      statically linked
```

And now when we run it in a system with an older `libc`, it works:

```
❯ docker run -it --rm roboxes/rhel8
❯ sudo docker cp target/x86_64-unknown-linux-musl/release/broker 0fd8973ab69c:/root/broker
[root@0fd8973ab69c ~]# ./broker --version
broker 0.2.1
```

We also get static linking of our other dependencies for free,
no more need to mess around with e.g. `libbz2`:

```
[root@0fd8973ab69c ~]# rm /usr/lib64/libbz2.so.1.0
rm: remove symbolic link '/usr/lib64/libbz2.so.1.0'? y
[root@0fd8973ab69c ~]# ./broker --version
broker 0.2.1
```

## Why `jemalloc`?

We created a benchmark to demonstrate the performance of a Rust program using `musl`.

Benchmark system information:
```
❯ macchina -o kernel -o processor -o memory
                                                                
     .--.       Kernel  -  6.3.1-arch1-1                        
    |o_o |      CPU     -  13th Gen Intel® Core™ i5-13600KF (20)
    |\_/ |      Memory  -  6.3 GB/65.7 GB                       
   //   \ \                                                     
  (|     | )                                                    
 /'\_   _/`\                                                    
 \___)=(___/ 
```

### `libc` with native allocator

This is considered the baseline.

```
❯ cargo bench
Running benches/allocations.rs (target/release/deps/allocations-16d4c626ab524c6e)
single thread           time:   [5.3980 ms 5.4019 ms 5.4057 ms]
                        change: [+0.6054% +0.7436% +0.8604%] (p = 0.00 < 0.05)
                        Change within noise threshold.
Found 3 outliers among 100 measurements (3.00%)
  3 (3.00%) high mild

multi thread            time:   [9.7086 ms 10.202 ms 10.725 ms]
                        change: [-13.696% -7.7467% -0.9965%] (p = 0.02 < 0.05)
                        Change within noise threshold.
Found 2 outliers among 100 measurements (2.00%)
  2 (2.00%) high mild
```

### `libc` with `jemalloc`

This is just for completion.

```
❯ cargo bench --features jemalloc
Running benches/allocations.rs (target/release/deps/allocations-c9acb5c19d58ded6)
single thread           time:   [5.6920 ms 5.6939 ms 5.6960 ms]
                        change: [+5.3211% +5.4061% +5.4884%] (p = 0.00 < 0.05)
                        Performance has regressed.
Found 3 outliers among 100 measurements (3.00%)
  2 (2.00%) low mild
  1 (1.00%) high severe

multi thread            time:   [10.329 ms 10.757 ms 11.213 ms]
                        change: [-1.0523% +5.4362% +12.550%] (p = 0.11 > 0.05)
                        No change in performance detected.
Found 4 outliers among 100 measurements (4.00%)
  4 (4.00%) high mild
```

### `musl` with native allocator

This is wildly slower than the baseline; this is why we use `jemalloc`.

```
❯ cross bench --target=x86_64-unknown-linux-musl
Running benches/allocations.rs (/target/x86_64-unknown-linux-musl/release/deps/allocations-7754353e43941b3f)
single thread           time:   [7.7555 ms 7.7674 ms 7.7898 ms]
                        change: [+44.223% +44.499% +44.930%] (p = 0.00 < 0.05)
                        Performance has regressed.
Found 11 outliers among 100 measurements (11.00%)
  8 (8.00%) high mild
  3 (3.00%) high severe

Benchmarking multi thread: Warming up for 3.0000 s
Warning: Unable to complete 100 samples in 5.0s. You may wish to increase target time to 7.8s, or reduce sample count to 60.
multi thread            time:   [74.947 ms 75.424 ms 75.896 ms]
                        change: [+567.90% +597.89% +628.02%] (p = 0.00 < 0.05)
                        Performance has regressed.
```

### `musl` with `jemalloc`

Here we see that `musl`, even with `jemalloc`, has a performance penalty but it's no longer a _597%_ increase to runtime.
This is an acceptable tradeoff for static linux binaries.

```
❯ cross bench --target=x86_64-unknown-linux-musl --features jemalloc
Running benches/allocations.rs (/target/x86_64-unknown-linux-musl/release/deps/allocations-ea9fccec62e130b2)
single thread           time:   [7.3118 ms 7.3198 ms 7.3328 ms]
                        change: [+36.207% +36.436% +36.724%] (p = 0.00 < 0.05)
                        Performance has regressed.
Found 4 outliers among 100 measurements (4.00%)
  2 (2.00%) high mild
  2 (2.00%) high severe

multi thread            time:   [12.961 ms 13.526 ms 14.122 ms]
                        change: [+21.107% +27.603% +35.021%] (p = 0.00 < 0.05)
                        Performance has regressed.
Found 2 outliers among 100 measurements (2.00%)
  2 (2.00%) high mild
```
