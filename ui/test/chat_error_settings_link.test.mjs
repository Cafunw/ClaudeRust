// npm run test:ui — a chat error that sends you to a Settings page ("set its path in Settings →
// Code") makes that page a link that opens it, in the UI's language.
import './stub_globals.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import { linkHelp } from '../js/parts.js';
import { codedMessage } from '../js/errors.js';
import { setLanguage } from '../js/i18n.js';
import { source } from './source.mjs';

test('the Settings page an error names is a link to it, in English and Turkish', () => {
  setLanguage('en');
  const en = linkHelp(codedMessage({ kind: 'cli_not_installed' }));
  assert.match(en, /<a href="#settings\/code">Settings → Code<\/a>/);
  assert.match(en, /<a href="https:\/\/claude\.com\/claude-code">claude\.com\/claude-code<\/a>/, 'the help site stays a link too');
  assert.match(linkHelp(codedMessage({ kind: 'mcp_not_running', params: { name: 'x' } })), /<a href="#settings\/developer">Settings → Developer<\/a>/);
  setLanguage('tr');
  assert.match(linkHelp(codedMessage({ kind: 'cli_not_installed' })), /<a href="#settings\/code">Ayarlar → Kod<\/a> bölümünde/);
  setLanguage('en');
  assert.equal(linkHelp('Nothing to link here.'), 'Nothing to link here.');
});

test('the link opens Settings on that page, from an app notice only', () => {
  const src = source('../js/parts.js');
  assert.match(src, /else if \(href\.startsWith\('#settings\/'\) && a\.closest\('\.notice'\)\) emit\('open-settings', href\.slice\('#settings\/'\.length\)\);/);
  // the error card puts the message through linkHelp
  assert.match(src, /<div>\$\{linkHelp\(esc\(msg\)\)\}<\/div>/);
});
