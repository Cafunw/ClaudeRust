// The text of a source file, for the tests that search it: with every "\r\n" made "\n", since a
// Windows checkout (core.autocrlf) has "\r\n" and a pattern with "\n" must still find its text.
import { readFileSync } from 'node:fs';

/** `rel`: a path relative to ui/test ('../js/chat.js'), or a file: URL. */
export const source = (rel) => readFileSync(new URL(rel, import.meta.url), 'utf8').replace(/\r\n/g, '\n');
