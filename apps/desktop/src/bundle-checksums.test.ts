import { createHash } from "node:crypto";
import { mkdir, mkdtemp, readFile, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { describe, expect, it } from "vitest";

import { writeBundleChecksums } from "../scripts/write-bundle-checksums.mjs";

function digest(value: string): string {
  return createHash("sha256").update(value).digest("hex");
}

describe("release bundle checksums", () => {
  it("sorts portable relative paths and replaces a previous manifest", async () => {
    const root = await mkdtemp(join(tmpdir(), "aetherstack-checksums-"));
    await mkdir(join(root, "nested"));
    await writeFile(join(root, "z-package.dmg"), "macOS");
    await writeFile(join(root, "nested", "a-package.deb"), "Linux");
    await writeFile(join(root, "nested", "bundle-helper.sh"), "not shipped");
    await writeFile(join(root, "SHA256SUMS.txt"), "stale absolute path");

    const destination = await writeBundleChecksums(root);
    const manifest = await readFile(destination, "utf8");

    expect(manifest).toBe(
      `${digest("Linux")}  nested/a-package.deb\n${digest("macOS")}  z-package.dmg\n`,
    );
    expect(manifest).not.toContain(root);
    expect(manifest).not.toContain("bundle-helper.sh");
  });

  it("fails when the bundle directory contains no distributable package", async () => {
    const root = await mkdtemp(join(tmpdir(), "aetherstack-checksums-"));
    await writeFile(join(root, "build-helper.sh"), "internal");

    await expect(writeBundleChecksums(root)).rejects.toThrow(
      "no distributable bundle artifacts found",
    );
  });
});
