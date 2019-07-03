use std::{
    fs::File,
    io::{BufReader, BufWriter},
    process::{Command, ExitStatus},
};

#[cfg(unix)]
use std::os::unix::process::ExitStatusExt;

#[cfg(target_os = "linux")]
use inferno::collapse::perf::{
    Folder, Options as CollapseOptions,
};

#[cfg(not(target_os = "linux"))]
use inferno::collapse::dtrace::{
    Folder, Options as CollapseOptions,
};

use inferno::{
    collapse::Collapse,
    flamegraph::{
        from_reader, Options as FlamegraphOptions,
    },
};

use signal_hook;

#[cfg(target_os = "linux")]
mod arch {
    use super::*;

    pub const SPAWN_ERROR: &'static str =
        "could not spawn perf";
    pub const WAIT_ERROR: &'static str =
        "unable to wait for perf \
         child command to exit";

    pub(crate) fn initial_command(
        workload: String,
    ) -> Command {
        let mut command = Command::new("perf");

        for arg in "record -F 99 --call-graph dwarf -g"
            .split_whitespace()
        {
            command.arg(arg);
        }

        for item in workload.split_whitespace() {
            command.arg(item);
        }

        command
    }

    pub fn output() -> Vec<u8> {
        Command::new("perf")
            .arg("script")
            .output()
            .expect("unable to call perf script")
            .stdout
    }
}

#[cfg(target_os = "linux")]
fn collapse_options() -> CollapseOptions {
    CollapseOptions::default()
}

#[cfg(not(target_os = "linux"))]
fn collapse_options() -> CollapseOptions {
    CollapseOptions {
        // We want the collapser to handle demangling since
        // DTrace doesn't do a good job demangling Rust names.
        demangle: true,
        ..Default::default()
    }
}

#[cfg(not(target_os = "linux"))]
mod arch {
    use super::*;

    pub const SPAWN_ERROR: &'static str =
        "could not spawn dtrace";
    pub const WAIT_ERROR: &'static str =
        "unable to wait for dtrace \
         child command to exit";

    pub(crate) fn initial_command(
        workload: String,
    ) -> Command {
        let mut command = Command::new("dtrace");

        let dtrace_script = "profile-997 /pid == $target/ \
                             { @[ustack(100)] = count(); }";

        // DTrace doesn't do a good job demangling
        // Rust names so do it in the collapser instead.
        command.arg("-xmangled");

        command.arg("-x");
        command.arg("ustackframes=100");

        command.arg("-n");
        command.arg(&dtrace_script);

        command.arg("-o");
        command.arg("cargo-flamegraph.stacks");

        command.arg("-c");
        command.arg(&workload);

        command
    }

    pub fn output() -> Vec<u8> {
        let mut buf = vec![];
        let mut f = File::open("cargo-flamegraph.stacks")
            .expect(
                "failed to open dtrace output \
                 file cargo-flamegraph.stacks",
            );

        use std::io::Read;
        f.read_to_end(&mut buf).expect(
            "failed to read dtrace expected \
             output file cargo-flamegraph.stacks",
        );

        std::fs::remove_file("cargo-flamegraph.stacks")
            .expect(
                "unable to remove cargo-flamegraph.stacks \
                 temporary file",
            );

        buf
    }
}

#[cfg(unix)]
fn terminated_by_error(status: ExitStatus) -> bool {
    status
        .signal() // the default needs to be true because that's the neutral element for `&&`
        .map_or(true, |code| {
            code != signal_hook::SIGINT
                && code != signal_hook::SIGTERM
        })
        && !status.success()
}

#[cfg(not(unix))]
fn terminated_by_error(status: ExitStatus) -> bool {
    !exit_status.success()
}

pub fn generate_flamegraph_by_running_command<
    P: AsRef<std::path::Path>,
>(
    workload: String,
    flamegraph_filename: P,
) {
    // Handle SIGINT with an empty handler. This has the
    // implicit effect of allowing the signal to reach the
    // process under observation while we continue to
    // generate our flamegraph.  (ctrl+c will send the
    // SIGINT signal to all processes in the foreground
    // process group).
    let handler = unsafe {
        signal_hook::register(signal_hook::SIGINT, || {})
            .expect("cannot register signal handler")
    };

    let mut command = arch::initial_command(workload);

    let mut recorder =
        command.spawn().expect(arch::SPAWN_ERROR);

    let exit_status =
        recorder.wait().expect(arch::WAIT_ERROR);

    signal_hook::unregister(handler);

    // only stop if perf exited unsuccessfully, but
    // was not killed by a signal (assuming that the
    // latter case usually means the user interrupted
    // it in some way)
    if terminated_by_error(exit_status) {
        eprintln!("failed to sample program");
        std::process::exit(1);
    }

    let output = arch::output();

    let perf_reader = BufReader::new(&*output);

    let mut collapsed = vec![];

    let collapsed_writer = BufWriter::new(&mut collapsed);

    Folder::from(collapse_options())
        .collapse(perf_reader, collapsed_writer)
        .expect(
            "unable to collapse generated profile data",
        );

    let collapsed_reader = BufReader::new(&*collapsed);

    println!(
        "writing flamegraph to {:?}",
        flamegraph_filename.as_ref()
    );

    let flamegraph_file = File::create(flamegraph_filename)
        .expect(
            "unable to create flamegraph.svg output file",
        );

    let flamegraph_writer = BufWriter::new(flamegraph_file);

    let mut flamegraph_options =
        FlamegraphOptions::default();

    from_reader(
        &mut flamegraph_options,
        collapsed_reader,
        flamegraph_writer,
    )
    .expect(
        "unable to generate a flamegraph \
         from the collapsed stack data",
    );
}
