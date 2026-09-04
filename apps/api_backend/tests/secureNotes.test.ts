import { describe, expect, test } from "bun:test";
import { generateMemorablePassword } from "../lib/passwordGenerator";
import { isSecretHandle, isSecureNoteCode } from "../lib/secureNotes";

describe("secret generation", () => {
  test("generates memorable passwords within requested constraints", () => {
    for (let index = 0; index < 25; index += 1) {
      const password = generateMemorablePassword({
        minWordLength: 4,
        maxWordLength: 7,
        maxPasswordLength: 18,
      });
      expect(password.length).toBeLessThanOrEqual(18);
      expect(password).toMatch(/^[A-Z][a-z]+[1-9][0-9][a-z]+[!#$%&*^]$/);
    }
  });

  test("does not use Math.random for password selection", () => {
    const original = Math.random;
    Math.random = () => {
      throw new Error("Math.random should not be used");
    };
    try {
      expect(generateMemorablePassword()).toMatch(/^[A-Z][a-z]+[1-9][0-9][a-z]+[!#$%&*^]$/);
    } finally {
      Math.random = original;
    }
  });

  test("rejects impossible password policies", () => {
    expect(() =>
      generateMemorablePassword({
        minWordLength: 6,
        maxWordLength: 3,
      }),
    ).toThrow("Minimum word length cannot be greater");
    expect(() =>
      generateMemorablePassword({
        minWordLength: 8,
        maxWordLength: 8,
        maxPasswordLength: 12,
      }),
    ).toThrow("too short");
  });
});

describe("secure note identifiers", () => {
  test("validates route codes and runner handles", () => {
    expect(isSecureNoteCode("a1b2c3d4")).toBe(true);
    expect(isSecureNoteCode("A1B2C3D4")).toBe(false);
    expect(isSecureNoteCode("a1b2c3d")).toBe(false);
    expect(isSecureNoteCode("a1b2c3d_")).toBe(false);

    expect(isSecretHandle("sec_a1b2c3d4e5f6g7h8")).toBe(true);
    expect(isSecretHandle("sec_a1b2c3d4e5f6g7H8")).toBe(false);
    expect(isSecretHandle("a1b2c3d4e5f6g7h8")).toBe(false);
  });
});
