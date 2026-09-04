import { randomInt } from "crypto";

type WordDictionary = {
  words: string[];
};

const wordDictionary = require("./wordDictionary.json") as WordDictionary;

export type PasswordOptions = {
  minWordLength?: number;
  maxWordLength?: number;
  maxPasswordLength?: number;
};

const DEFAULT_MIN_WORD_LENGTH = 3;
const DEFAULT_MAX_WORD_LENGTH = 10;
const SPECIAL_CHARACTERS = ["!", "#", "$", "%", "&", "*", "^"];
const wordList = wordDictionary.words.filter((word) => /^[a-z]+$/i.test(word));

function boundedInt(min: number, max: number): number {
  return randomInt(min, max + 1);
}

function randomItem<T>(items: T[]): T {
  if (items.length === 0) {
    throw new Error("No values are available for random selection");
  }
  return items[randomInt(0, items.length)];
}

function capitalizeFirstLetter(word: string): string {
  return word.charAt(0).toUpperCase() + word.slice(1).toLowerCase();
}

function filteredWords(minLength: number, maxLength: number): string[] {
  return wordList.filter((word) => word.length >= minLength && word.length <= maxLength);
}

function normalizeOptions(options: PasswordOptions = {}) {
  const minWordLength = Math.max(1, Math.trunc(options.minWordLength ?? DEFAULT_MIN_WORD_LENGTH));
  const maxWordLength = Math.max(1, Math.trunc(options.maxWordLength ?? DEFAULT_MAX_WORD_LENGTH));
  const maxPasswordLength =
    options.maxPasswordLength === undefined || options.maxPasswordLength === null
      ? null
      : Math.max(1, Math.trunc(options.maxPasswordLength));
  if (minWordLength > maxWordLength) {
    throw new Error("Minimum word length cannot be greater than maximum word length");
  }
  return { minWordLength, maxWordLength, maxPasswordLength };
}

export function generateMemorablePassword(options: PasswordOptions = {}): string {
  const { minWordLength, maxWordLength, maxPasswordLength } = normalizeOptions(options);
  const words = filteredWords(minWordLength, maxWordLength);
  if (words.length === 0) {
    throw new Error(`No words found with length between ${minWordLength} and ${maxWordLength}`);
  }

  const fixedCharsLength = 3;
  if (maxPasswordLength !== null) {
    const maxCombinedWordLength = maxPasswordLength - fixedCharsLength;
    if (maxCombinedWordLength < minWordLength * 2) {
      throw new Error("Max password length is too short to generate a valid password");
    }

    for (let attempt = 0; attempt < 100; attempt += 1) {
      const maxFirstWordLength = Math.min(maxWordLength, maxCombinedWordLength - minWordLength);
      const firstWord = randomItem(words.filter((word) => word.length <= maxFirstWordLength));
      const remainingLength = maxCombinedWordLength - firstWord.length;
      const secondWord = randomItem(words.filter((word) => word.length <= remainingLength));
      const password = `${capitalizeFirstLetter(firstWord)}${boundedInt(10, 99)}${secondWord.toLowerCase()}${randomItem(SPECIAL_CHARACTERS)}`;
      if (password.length <= maxPasswordLength) {
        return password;
      }
    }
    throw new Error(`Unable to generate password within ${maxPasswordLength} characters with current word length settings`);
  }

  const firstWord = randomItem(words);
  const secondWord = randomItem(words);
  return `${capitalizeFirstLetter(firstWord)}${boundedInt(10, 99)}${secondWord.toLowerCase()}${randomItem(SPECIAL_CHARACTERS)}`;
}
