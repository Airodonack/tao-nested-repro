// The page: a clock of its own, the backend's ticks, two invoke round trips,
// and the trigger buttons. Anything over 500 ms late goes in the log.

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const $ = (id) => document.getElementById(id);
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const STALL_MS = 500;
// Ticks that arrive this close together after a stall belong to its burst.
const BURST_GAP_MS = 50;

const logLines = [];
function log(line) {
  const stamp = new Date().toISOString().slice(11, 23);
  logLines.push(`${stamp}  ${line}`);
  $("log").textContent = logLines.join("\n");
  $("log").scrollTop = $("log").scrollHeight;
  console.log(line);
}

// 1. The page clock.
let clock = 0;
setInterval(() => {
  clock += 1;
  $("clock").textContent = clock;
  const since = performance.now() - lastTick;
  $("since").textContent = lastTick ? `${(since / 1000).toFixed(1)} s` : "-";
  $("ticks").classList.toggle("stalled", lastTick > 0 && since > STALL_MS);
  for (const probe of probes) {
    if (probe.since) {
      $(probe.el).textContent = `${((performance.now() - probe.since) / 1000).toFixed(1)} s…`;
    }
  }
}, 100);

// 2. The backend's ticks.
let ticks = 0;
let lastTick = 0;
let burst = null;
function endBurst() {
  if (!burst) return;
  const { stall, count, first, last } = burst;
  burst = null;
  log(
    `stalled ${(stall / 1000).toFixed(1)} s, then ${count} ticks arrived within ${Math.round(last - first)} ms`,
  );
}
listen("tick", () => {
  const now = performance.now();
  ticks += 1;
  $("ticks").textContent = ticks;
  if (burst && now - burst.last <= BURST_GAP_MS) {
    burst.count += 1;
    burst.last = now;
  } else {
    endBurst();
    if (lastTick && now - lastTick > STALL_MS) {
      burst = { stall: now - lastTick, count: 1, first: now, last: now };
      setTimeout(endBurst, 300);
    }
  }
  lastTick = now;
});

// 3. The invoke round trips: a plain command, whose answer reaches the page
// through a GLib source, and one that asks the window its size first, which
// goes through tao's queue.
const probes = [
  { name: "plain invoke", cmd: "ask_backend", el: "rtt", since: 0, trips: null },
  { name: "window size", cmd: "ask_window", el: "rtt-window", since: 0, trips: null },
];
async function ask(probe) {
  if (probe.since) return;
  probe.since = performance.now();
  await invoke(probe.cmd);
  const took = performance.now() - probe.since;
  probe.since = 0;
  $(probe.el).textContent = `${Math.round(took)} ms`;
  if (probe.trips) {
    probe.trips.count += 1;
    probe.trips.slowest = Math.max(probe.trips.slowest, took);
  }
  if (took > STALL_MS) log(`${probe.name}: round trip took ${(took / 1000).toFixed(1)} s`);
}
$("ask").addEventListener("click", () => ask(probes[0]));
$("ask-window").addEventListener("click", () => ask(probes[1]));
let autoAsk = 0;
function setAutoAsk(on) {
  $("auto-ask").checked = on;
  clearInterval(autoAsk);
  if (on) autoAsk = setInterval(() => probes.forEach(ask), 500);
}
$("auto-ask").addEventListener("change", (e) => setAutoAsk(e.target.checked));

// 4. The triggers.
async function trigger(cmd) {
  const started = performance.now();
  log(`${cmd}: started`);
  for (const probe of probes) probe.trips = { count: 0, slowest: 0 };
  let result;
  try {
    result = await invoke(cmd);
  } catch (e) {
    result = `failed: ${e}`;
  }
  const took = ((performance.now() - started) / 1000).toFixed(1);
  log(`${cmd}: returned after ${took} s${result ? `, ${result}` : ""}`);
  for (const probe of probes) {
    const { count, slowest } = probe.trips;
    if (count) log(`${probe.name} meanwhile: ${count} round trips, slowest ${Math.round(slowest)} ms`);
    probe.trips = null;
  }
}
for (const button of document.querySelectorAll("[data-cmd]")) {
  button.addEventListener("click", () => trigger(button.dataset.cmd));
}

// Unattended runs (REPRO_AUTO=<command>): press the trigger after 2 s, then
// hand the log back and quit.
(async () => {
  const auto = await invoke("auto_mode");
  if (!auto) return;
  setAutoAsk(true);
  await sleep(2000);
  await trigger(auto);
  await sleep(4000);
  await invoke("finish", { log: logLines.join("\n") + "\n" });
})();
