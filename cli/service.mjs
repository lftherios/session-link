import { mkdir, writeFile, rm } from "node:fs/promises";
import { spawnSync } from "node:child_process";
import os from "node:os";
import path from "node:path";

/**
 * Install the always-on tap as a user login service so capture survives
 * reboots and starts on its own — the piece that makes `slink tap` truly
 * "set once." launchd on macOS, systemd --user on Linux.
 *
 * The plist/unit generators are pure (and unit-tested); install/uninstall
 * shell out to launchctl/systemctl and are only ever run by the user via
 * `slink tap --install`.
 */

export const SERVICE_LABEL = "link.session.tap";

const escapeXml = (s) =>
  String(s).replace(/[<>&'"]/g, (c) => ({ "<": "&lt;", ">": "&gt;", "&": "&amp;", "'": "&apos;", '"': "&quot;" })[c]);

/** Absolute node + CLI entry so the service never depends on the login PATH. */
export function tapProgramArgs(port = 4141) {
  return [process.execPath, process.argv[1], "tap", "--port", String(port)];
}

export function launchdPlist({ programArgs, logPath, label = SERVICE_LABEL }) {
  const args = programArgs.map((a) => `      <string>${escapeXml(a)}</string>`).join("\n");
  return `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
  <dict>
    <key>Label</key>
    <string>${label}</string>
    <key>ProgramArguments</key>
    <array>
${args}
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>${escapeXml(logPath)}</string>
    <key>StandardErrorPath</key>
    <string>${escapeXml(logPath)}</string>
  </dict>
</plist>
`;
}

export function systemdUnit({ programArgs }) {
  const exec = programArgs.map((a) => (/\s/.test(a) ? `"${a}"` : a)).join(" ");
  return `[Unit]
Description=session.link always-on tap
After=network.target

[Service]
ExecStart=${exec}
Restart=always

[Install]
WantedBy=default.target
`;
}

function launchdPath() {
  return path.join(os.homedir(), "Library", "LaunchAgents", `${SERVICE_LABEL}.plist`);
}
function systemdPath() {
  return path.join(os.homedir(), ".config", "systemd", "user", "slink-tap.service");
}

export async function installService({ port = 4141 } = {}) {
  const programArgs = tapProgramArgs(port);
  if (process.platform === "darwin") {
    const plistPath = launchdPath();
    const logPath = path.join(os.homedir(), ".slink", "tap.log");
    await mkdir(path.dirname(plistPath), { recursive: true });
    await mkdir(path.dirname(logPath), { recursive: true });
    await writeFile(plistPath, launchdPlist({ programArgs, logPath }));
    spawnSync("launchctl", ["unload", plistPath], { stdio: "ignore" });
    const r = spawnSync("launchctl", ["load", "-w", plistPath], { encoding: "utf8" });
    return { ok: r.status === 0, path: plistPath, error: r.error?.message || r.stderr?.trim() };
  }
  if (process.platform === "linux") {
    const unitPath = systemdPath();
    await mkdir(path.dirname(unitPath), { recursive: true });
    await writeFile(unitPath, systemdUnit({ programArgs }));
    spawnSync("systemctl", ["--user", "daemon-reload"], { stdio: "ignore" });
    const r = spawnSync("systemctl", ["--user", "enable", "--now", "slink-tap.service"], { encoding: "utf8" });
    return { ok: r.status === 0, path: unitPath, error: r.error?.message || r.stderr?.trim() };
  }
  return { ok: false, error: `service install isn't supported on ${process.platform} yet` };
}

export async function uninstallService() {
  if (process.platform === "darwin") {
    const plistPath = launchdPath();
    spawnSync("launchctl", ["unload", "-w", plistPath], { stdio: "ignore" });
    await rm(plistPath, { force: true });
    return { ok: true, path: plistPath };
  }
  if (process.platform === "linux") {
    const unitPath = systemdPath();
    spawnSync("systemctl", ["--user", "disable", "--now", "slink-tap.service"], { stdio: "ignore" });
    await rm(unitPath, { force: true });
    return { ok: true, path: unitPath };
  }
  return { ok: false, error: `service uninstall isn't supported on ${process.platform} yet` };
}
