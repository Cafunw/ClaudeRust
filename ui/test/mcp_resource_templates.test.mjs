// npm run test:ui — MCP resource templates (ui/js/mcp_resources.js): their variables are asked for,
// then put into the URI as RFC 6570 expands them (the cases of its section 3.2 that apply to text).
import test from 'node:test';
import assert from 'node:assert/strict';
import { templateVars, expandTemplate, templateForm } from '../js/mcp_resources.js';

const v = { var: 'value', hello: 'Hello World!', path: '/foo/bar', x: '1024', y: '768', empty: '', keys: 'a,b' };

test('the operators of RFC 6570, as its examples expand them', () => {
  const cases = [
    ['{var}', 'value'], ['{hello}', 'Hello%20World%21'], ['{+hello}', 'Hello%20World!'], ['{+path}/here', '/foo/bar/here'],
    ['{#hello}', '#Hello%20World!'], ['X{.var}', 'X.value'], ['{/var,x}/here', '/value/1024/here'], ['{;x,y}', ';x=1024;y=768'],
    ['{?x,y}', '?x=1024&y=768'], ['?fixed=yes{&x}', '?fixed=yes&x=1024'], ['{var:3}', 'val'], ['{x,hello,y}', '1024,Hello%20World%21,768'],
    ['map?{x,y}', 'map?1024,768'], ['{+path:6}/here', '/foo/b/here'],
  ];
  for (const [tpl, want] of cases) assert.equal(expandTemplate(tpl, v), want, tpl);
});

test('a variable left out (or empty) is skipped, with what would have introduced it', () => {
  assert.equal(expandTemplate('db://users/{id}{?fields,limit}', { id: '42' }), 'db://users/42');
  assert.equal(expandTemplate('db://users/{id}{?fields,limit}', { id: '42', limit: '5' }), 'db://users/42?limit=5');
  assert.equal(expandTemplate('repo://{owner}/{name}/file/{+path}', { owner: 'a b', name: 'r/x', path: 'src/main.rs' }), 'repo://a%20b/r%2Fx/file/src/main.rs');
  assert.equal(expandTemplate('{empty}x', v), 'x');
  assert.equal(expandTemplate('no variables', v), 'no variables');
});

test('the variables are asked for once each, a path one as required, a query one as optional', () => {
  assert.deepEqual(templateVars('repo://{owner}/{name}{/path*}{?ref,depth}{#frag}{owner}'), [
    { name: 'owner', required: true }, { name: 'name', required: true }, { name: 'path', required: true },
    { name: 'ref', required: false }, { name: 'depth', required: false }, { name: 'frag', required: false },
  ]);
  assert.deepEqual(templateVars(undefined), []);
});

test('a template is filled in like a prompt: its title, the template under its description', () => {
  const f = templateForm({ server: 's', uriTemplate: 'db://users/{id}', name: 'user', title: 'A user', description: 'One user by id' });
  assert.deepEqual(f, { name: 'A user', description: 'One user by id · db://users/{id}', arguments: [{ name: 'id', required: true }] });
  assert.equal(templateForm({ uriTemplate: 't://{a}', name: 'n' }).name, 'n');
});
