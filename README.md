<p align="center"><img src="ui/icon.svg" width="72" height="72" alt=""></p>

<h1 align="center">spanner</h1>

<p align="center">Tune and monitor a Windows laptop: power, fans and charging in one small window, with a recorded history of how the machine actually runs.</p>

---

spanner replaces the parts of a vendor control centre you actually use with a single small program. It sets the processor's power behaviour, fan mode, Cooler Boost and battery charge limit. It shows temperatures, clocks, load, fan speeds and GPU throttling live. A background recorder keeps every reading, so you can see what happened during last night's session rather than guess.

Power settings and monitoring work on any Windows laptop. Fan modes, performance profiles and charge limits come from vendor drivers that speak the same firmware interfaces as the manufacturer's own software; the first driver supports MSI. It does not install drivers, run a service of its own, or connect to the internet.

## Contents

- [Features](#features)
- [Supported hardware](#supported-hardware)
- [Install](#install)
- [Using spanner](#using-spanner)
- [How it stays safe](#how-it-stays-safe)
- [Data and privacy](#data-and-privacy)
- [Uninstall](#uninstall)
- [Architecture](#architecture)
- [Development](#development)

## Features

**Configure.** Switches, choices and sliders that take effect as soon as you change them, with Undo in the confirmation. A **Reset** link appears next to anything not at its default. The **Presets** menu applies a whole set at once (Gaming and Defaults are built in), ticks the one that matches your current settings, and saves your own: choose **Save current settings as preset…** and give it a name. After applying a preset, expand the confirmation to see each change. Technical processor settings sit under **Advanced**.

| Section     | Setting                    | Available on | What it does                                                   |
|-------------|----------------------------|--------------|----------------------------------------------------------------|
| Performance | Turbo boost                | Any machine  | Lets the CPU run above its base clock; off runs much cooler    |
|             | Power mode                 | MSI          | Eco, Balanced or Turbo power limits for the CPU and GPU        |
|             | Advanced                   | Any machine  | Processor state limits, turbo boost mode, energy preference    |
| Cooling     | Cool before slowing down   | Any machine  | Raises fan speed before reducing CPU speed                     |
|             | Fan profile                | MSI          | Silent, Auto, or Advanced (the curve set in MSI Center)        |
|             | Full-speed fans            | MSI          | Off, When hot (on at CPU 90 °C or GPU 83 °C, off below 80/73 °C), or Always |
| Battery     | Charge limit               | MSI          | Stops charging at 50–100% to slow battery wear                 |

**Live.** Temperatures, load, clocks, GPU power and memory, fan speeds and battery, with a short trend under each. It refreshes every 5 seconds by default; the selector at the top offers 1, 5, 15, 30 or 60 seconds, or any interval up to 10 minutes. Figures turn cobalt when something needs attention, such as a CPU near its thermal limit or a GPU being held back by heat or power.

**History.** Every reading the recorder takes, charted as a minimum to maximum band with the average drawn through it. Filter by time range, by mains or battery power, and by **Data from**: everything, any tracked app, or one game, listed with how long it ran in the range. Each chart shows the average, 95th percentile and maximum for the range, and hovering one chart moves the cursor on all of them. To zoom, drag across a chart, hold Ctrl and scroll, or use the − and + buttons (or the `-`, `+` and `0` keys).

## Supported hardware

**Every Windows 10 or 11 laptop** gets the processor and cooling-policy settings, CPU load and clock, temperature from the ACPI thermal zones, battery, memory, and the full history. GPU figures need an Nvidia GPU and driver.

**Vendor drivers** add fan control, performance modes, charge limits and the more accurate embedded-controller temperatures. A driver only activates on hardware it has been verified on; anywhere else spanner simply does not show those settings.

| Vendor | Model                 | EC firmware    | Status   |
|--------|-----------------------|----------------|----------|
| MSI    | Stealth 15M B12UE     | `15B1EMS1.1xx` | Verified |

Other MSI models are a table entry in `src/hw/msi.rs` once verified on the machine. Other vendors (Lenovo, ASUS, Dell, HP each expose their own firmware interface) are a new driver; see [Architecture](#architecture).

## Install

spanner is built from source.

**Requirements**

- Windows 10 or 11, x64
- [Rust](https://rustup.rs) (the repository pins its version in `rust-toolchain.toml`; rustup installs it automatically)
- Visual Studio Build Tools with the "Desktop development with C++" workload
- Microsoft Edge WebView2 Runtime (included with Windows 11)

**Build and register**

```powershell
git clone https://github.com/Felix-Hz/spanner.git
cd spanner
cargo build --release
.\target\release\spanner.exe --install
```

`--install` asks for administrator approval once, then:

- registers the background recorder to start at logon,
- adds spanner to **Settings › Apps › Installed apps**,
- puts a **spanner** shortcut on your Desktop.

Running `--install` again is safe; it refreshes all three.

## Using spanner

Open the Desktop shortcut. The window runs as you, with no approval prompt: reading the embedded controller and changing power plans happen in the background recorder, which Task Scheduler starts with administrator rights when you sign in. If the recorder isn't running when you open spanner, the window starts it.

| Page      | Use it to                                                                         |
|-----------|-----------------------------------------------------------------------------------|
| Configure | Change a setting; it applies straight away, and the confirmation offers Undo      |
| Live      | Watch the machine right now                                                       |
| History   | Look back over any range, filtered by power source or by tracked apps             |
| Settings  | Theme, what the close button does, background recording and tracked apps, presets, and Uninstall |

`Ctrl+1` to `Ctrl+4` switch pages. Closing the window minimises spanner to the notification area by default; click its icon to open it again, or right-click and choose **Quit spanner**. Settings › Appearance › Close button changes this to quit instead. The status bar shows the power source, whether the recorder is running, the power plan and the machine. spanner remembers its window size and position, and opening it again brings the existing window forward.

### Tracked apps

The recorder marks each sample taken while a tracked app is running. Add your games under **Settings › Background recording › Tracked apps**: a name (optional) and the program file (for example `PlanetZoo.exe`). Then pick the game under **History › Data from** to see how the machine behaves while it runs.

### Background recording

**Record in the background** turns recording on or off straight away. **Start when you sign in** controls whether the recorder starts by itself each time you sign in to Windows. With recording off the recorder keeps running so the window can still read and change settings, but it writes no history. Full-speed fans **When hot** keeps working either way.

## How it stays safe

Writing the wrong byte to an embedded controller can misbehave in ways that are hard to diagnose, so spanner is deliberately narrow:

- **A fixed list of writes.** Each vendor driver can only change the settings it declares. There is no general "write any address" path anywhere in the code.
- **Verified hardware only.** A driver activates only when the firmware matches a map verified on real hardware; otherwise its settings are not offered at all.
- **Read-back.** Every controller write is read back; if the value did not take, you are told.
- **Validation before writing.** Every change is checked against its allowed values first; a preset with one invalid value writes nothing.
- **Bounds.** The charge limit cannot be set below 50%.
- **A sealed window.** The interface is compiled into the program, may not load anything from the network, and can only send a fixed set of typed requests to the backend.
- **An unprivileged window.** The window never runs as administrator. It sends hardware requests to the recorder over a local pipe that only your account, SYSTEM and administrators can open, and that refuses remote connections. The recorder accepts only spanner's own requests and validates each one, and the window checks that the pipe belongs to spanner before trusting a reply.

Vendor software (MSI Center, Lenovo Vantage and similar) writes some of the same settings. If both are installed, whichever changed a setting last wins; pick one to manage fans and charging.

## Data and privacy

Everything stays on your machine.

| What                                       | Where                                   |
|--------------------------------------------|-----------------------------------------|
| Recorded history (one file per day)        | `%ProgramData%\spanner\history`         |
| Recorder settings                          | `%ProgramData%\spanner\recorder.json`   |
| Window preferences and web view cache      | `%LOCALAPPDATA%\spanner`                |

The recorder samples every 5 seconds by default and keeps 30 days, about 21 MB. It runs at below-normal priority. On battery it leaves the Nvidia GPU asleep unless something else has already woken it, so recording does not cost battery life.

## Uninstall

Either:

- **Settings › Apps › Installed apps › spanner › Uninstall**, or
- **Settings › Uninstall** inside spanner, which can also restore default settings first.

Both remove the recorder, all recorded history, the Desktop shortcut and the Installed apps entry. Settings you applied stay as they are unless you chose to restore defaults. The repository and build are yours to delete.

## Architecture

```
src/
├── engine/            portable: storage, queries, settings model, sampling, UI protocol
├── hw/                vendor drivers: one per manufacturer, detected at start-up
│   └── msi.rs         MSI embedded-controller maps, one entry per verified firmware family
├── platform/          the OS boundary: traits every platform implements
│   └── windows/       WMI, power plans, PDH counters, Task Scheduler, registry
├── app.rs             window and threads
├── recorder.rs        background recorder and the window's broker
└── service.rs         requests that need administrator rights, served by the broker
ui/                    HTML, CSS and JavaScript, embedded at build time
```

- **Vendor drivers.** A driver implements the `Vendor` trait in `src/hw/mod.rs`: detect its hardware, report readings, declare its settings and read or write them. Drivers never see the operating system directly, only the transports `platform/` lends them, so one driver serves every OS.
- **Portable by construction.** Only `platform/` touches the operating system. Linux and macOS support means adding `platform/linux/` or `platform/macos/` that implements the traits in `platform/mod.rs`; the table there lists the native mechanism for each concern. The interface (`wry` and `tao`) and the GPU library (NVML) already run on all three.
- **Storage.** History is append-only, one file per UTC day, made of fixed 40-byte records in time order. A query binary-searches to the start of the range and then reads sequentially, so memory use stays constant however long the range.
- **Charts.** Each query is reduced in one pass to one bucket per pixel of chart width, keeping minimum, maximum and mean, so a 30-day chart is as fast as a one-hour one and still shows every spike. Percentiles come from a fixed-bin histogram built in the same pass.
- **Privileges.** Only the recorder is elevated, by its Task Scheduler entry. Its broker thread serves the window over a named pipe (`platform/windows/pipe.rs`); presets and history are the user's own files and stay in the window. `--install` and `--uninstall` relaunch themselves elevated when needed.
- **Threads.** The window never waits: broker requests run on a worker thread, and history queries on their own thread, where a newer query replaces any that has not started.

## Development

```powershell
cargo test                       # unit tests: storage, queries, settings, EC map, protocol
cargo build                      # debug build; web view developer tools enabled
cargo build --release            # optimised, stripped build
pwsh scripts/rebuild.ps1 -Open   # rebuild while installed: stops and restarts the recorder
python -I scripts/make_icon.py   # regenerate the icon from its pixel grid
```

The icon is drawn on a 16-pixel grid in `scripts/make_icon.py` and scaled up without smoothing, matching the type and colours of the interface.

## Acknowledgements

- Embedded-controller addresses: [msi-ec](https://github.com/BeardOverflow/msi-ec).
- Typefaces: [Newsreader](https://github.com/productiontype/Newsreader), [Instrument Sans](https://github.com/Instrument/instrument-sans) and [JetBrains Mono](https://github.com/JetBrains/JetBrainsMono) under the SIL Open Font License 1.1, and [Departure Mono](https://departuremono.com) under the MIT License. Their licences are in `ui/fonts/`; the bundled files are Latin subsets.

## Licence

MIT. See [LICENSE](LICENSE).
