// MCP resource templates (resources/templates/list): a URI template (RFC 6570) whose variables the
// person fills in before the resource is read. Variables of the path are asked for as required,
// those of a query or fragment as optional (left out when empty).

// RFC 6570 section 3.2: per operator, what starts the expansion, what joins its values, whether
// values are named, what a named empty value gets, whether reserved characters pass unencoded
const OPS = {
  '': ['', ',', false, '', false],
  '+': ['', ',', false, '', true],
  '#': ['#', ',', false, '', true],
  '.': ['.', '.', false, '', false],
  '/': ['/', '/', false, '', false],
  ';': [';', ';', true, '', false],
  '?': ['?', '&', true, '=', false],
  '&': ['&', '&', true, '=', false],
};
const EXPR = /\{([+#./;?&]?)([^{}]*)\}/g;
const VAR = /^([A-Za-z0-9_.%]+)(?::(\d{1,4}))?\*?$/;

/** The variables of URI template `tpl`, in order and once each: [{name, required}]. */
export function templateVars(tpl) {
  const seen = new Map();
  for (const [, op, list] of String(tpl || '').matchAll(EXPR)) {
    for (const spec of list.split(',')) {
      const m = VAR.exec(spec.trim());
      if (m && !seen.has(m[1])) seen.set(m[1], !['?', '&', '#'].includes(op));
    }
  }
  return [...seen].map(([name, required]) => ({ name, required }));
}

const encode = (v, reserved) => (reserved
  // reserved characters and %XX triplets as they are
  ? encodeURI(v).replace(/%25([0-9A-Fa-f]{2})/g, '%$1')
  : encodeURIComponent(v).replace(/[!'()*]/g, (c) => `%${c.charCodeAt(0).toString(16).toUpperCase()}`));

/** URI template `tpl` with the variables of `values` (name -> text) put in; one left out is skipped. */
export function expandTemplate(tpl, values) {
  return String(tpl || '').replace(EXPR, (_, op, list) => {
    const [first, sep, named, ifEmpty, reserved] = OPS[op];
    const parts = [];
    for (const spec of list.split(',')) {
      const m = VAR.exec(spec.trim());
      const raw = m && values[m[1]];
      if (raw === undefined || raw === null || raw === '') continue;
      const v = encode(m[2] ? String(raw).slice(0, Number(m[2])) : String(raw), reserved);
      parts.push(named ? `${m[1]}${v === '' ? ifEmpty : `=${v}`}` : v);
    }
    return parts.length ? first + parts.join(sep) : '';
  });
}

/** Template `res` (as mcp_list_prompts lists it) as a prompt to fill in: its title, its template under its description, its variables. */
export function templateForm(res) {
  return {
    name: res.title || res.name || res.uriTemplate,
    description: [res.description, res.uriTemplate].filter(Boolean).join(' · '),
    arguments: templateVars(res.uriTemplate),
  };
}
