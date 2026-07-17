import assert from "node:assert/strict";
import { test } from "node:test";
import { launchdPlist, systemdUnit, tapProgramArgs, SERVICE_LABEL } from "../cli/service.mjs";

test("tapProgramArgs: absolute node + CLI entry, tap on the given port", () => {
  const args = tapProgramArgs(4242);
  assert.equal(args[0], process.execPath);
  assert.equal(args[1], process.argv[1]);
  assert.deepEqual(args.slice(2), ["tap", "--port", "4242"]);
});

test("launchdPlist: label, program args, run-at-load, keepalive, log path", () => {
  const plist = launchdPlist({
    programArgs: ["/usr/bin/node", "/opt/slink/cli.mjs", "tap", "--port", "4141"],
    logPath: "/Users/e/.slink/tap.log",
  });
  assert.match(plist, /<key>Label<\/key>\s*<string>link\.session\.tap<\/string>/);
  assert.match(plist, /<string>tap<\/string>/);
  assert.match(plist, /<string>--port<\/string>\s*<string>4141<\/string>/);
  assert.match(plist, /<key>RunAtLoad<\/key>\s*<true\/>/);
  assert.match(plist, /<key>KeepAlive<\/key>\s*<true\/>/);
  assert.match(plist, /<string>\/Users\/e\/\.slink\/tap\.log<\/string>/);
  assert.equal(SERVICE_LABEL, "link.session.tap");
});

test("launchdPlist: escapes XML-special characters in arguments", () => {
  const plist = launchdPlist({
    programArgs: ["/bin/node", "/weird & <path>/cli.mjs", "tap"],
    logPath: "/tmp/l.log",
  });
  assert.match(plist, /weird &amp; &lt;path&gt;/);
  assert.doesNotMatch(plist, /weird & <path>/);
});

test("systemdUnit: ExecStart with the args, restart=always, install target", () => {
  const unit = systemdUnit({ programArgs: ["/usr/bin/node", "/opt/slink/cli.mjs", "tap", "--port", "4141"] });
  assert.match(unit, /ExecStart=\/usr\/bin\/node \/opt\/slink\/cli\.mjs tap --port 4141/);
  assert.match(unit, /Restart=always/);
  assert.match(unit, /WantedBy=default\.target/);
});

test("systemdUnit: quotes arguments containing spaces", () => {
  const unit = systemdUnit({ programArgs: ["/bin/node", "/a b/cli.mjs", "tap"] });
  assert.match(unit, /ExecStart=\/bin\/node "\/a b\/cli\.mjs" tap/);
});
