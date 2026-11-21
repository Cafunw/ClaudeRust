// Stand-ins for the shadcn/ui components React artifacts import from
// "@/components/ui/*", and cn() from "@/lib/utils": the same names, props
// and Tailwind classes, drawn against the theme variables of the artifact
// document (artifacts.rs). Plain JS: the page runs it without a build step.
import * as React from 'react';
import { clsx } from 'clsx';
import { twMerge } from 'tailwind-merge';

const h = React.createElement;

export function cn(...inputs) {
  return twMerge(clsx(inputs));
}

const chain = (a, b) => (a && b ? (e) => { a(e); b(e); } : a || b);

// `tag` with `props`, or with asChild the one child element given these props
function slot(asChild, tag, props) {
  const { children, ...rest } = props;
  if (asChild && React.isValidElement(children)) {
    const cp = children.props;
    return React.cloneElement(children, { ...rest, ...cp, className: cn(rest.className, cp.className), onClick: chain(rest.onClick, cp.onClick) });
  }
  return h(tag, props, children);
}

// a component drawing `tag` with the base classes and any className added
function el(tag, base, name) {
  const C = React.forwardRef(({ className, ...props }, ref) => h(tag, { ref, className: cn(base, className), ...props }));
  C.displayName = name;
  return C;
}

// a value the caller may own (value + onChange) or leave to the component
function useControlled(value, initial, onChange) {
  const [inner, setInner] = React.useState(initial);
  const own = value === undefined;
  const set = (v) => { if (own) setInner(v); if (onChange) onChange(v); };
  return [own ? inner : value, set];
}

const BUTTON = {
  variant: {
    default: 'bg-primary text-primary-foreground hover:bg-primary/90',
    destructive: 'bg-destructive text-destructive-foreground hover:bg-destructive/90',
    outline: 'border border-input bg-background hover:bg-accent hover:text-accent-foreground',
    secondary: 'bg-secondary text-secondary-foreground hover:bg-secondary/80',
    ghost: 'hover:bg-accent hover:text-accent-foreground',
    link: 'text-primary underline-offset-4 hover:underline',
  },
  size: { default: 'h-10 px-4 py-2', sm: 'h-9 rounded-md px-3', lg: 'h-11 rounded-md px-8', icon: 'h-10 w-10' },
};

export function buttonVariants({ variant = 'default', size = 'default', className } = {}) {
  return cn('inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md text-sm font-medium ring-offset-background transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 disabled:pointer-events-none disabled:opacity-50 [&_svg]:size-4 [&_svg]:shrink-0',
    BUTTON.variant[variant] || BUTTON.variant.default, BUTTON.size[size] || BUTTON.size.default, className);
}

export const Button = React.forwardRef(({ className, variant, size, asChild, ...props }, ref) =>
  slot(asChild, 'button', { ref, className: buttonVariants({ variant, size, className }), ...props }));
Button.displayName = 'Button';

const BADGE = {
  default: 'border-transparent bg-primary text-primary-foreground hover:bg-primary/80',
  secondary: 'border-transparent bg-secondary text-secondary-foreground hover:bg-secondary/80',
  destructive: 'border-transparent bg-destructive text-destructive-foreground hover:bg-destructive/80',
  outline: 'text-foreground',
};

export function badgeVariants({ variant = 'default', className } = {}) {
  return cn('inline-flex items-center rounded-full border px-2.5 py-0.5 text-xs font-semibold transition-colors', BADGE[variant] || BADGE.default, className);
}

export function Badge({ className, variant, ...props }) {
  return h('div', { className: badgeVariants({ variant, className }), ...props });
}

export const Card = el('div', 'rounded-lg border bg-card text-card-foreground shadow-sm', 'Card');
export const CardHeader = el('div', 'flex flex-col space-y-1.5 p-6', 'CardHeader');
export const CardTitle = el('h3', 'text-2xl font-semibold leading-none tracking-tight', 'CardTitle');
export const CardDescription = el('p', 'text-sm text-muted-foreground', 'CardDescription');
export const CardContent = el('div', 'p-6 pt-0', 'CardContent');
export const CardFooter = el('div', 'flex items-center p-6 pt-0', 'CardFooter');

const FIELD = 'w-full rounded-md border border-input bg-background px-3 py-2 text-sm ring-offset-background placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 disabled:cursor-not-allowed disabled:opacity-50';
export const Input = el('input', `flex h-10 ${FIELD} file:border-0 file:bg-transparent file:text-sm file:font-medium`, 'Input');
export const Textarea = el('textarea', `flex min-h-[80px] ${FIELD}`, 'Textarea');
export const Label = el('label', 'text-sm font-medium leading-none peer-disabled:cursor-not-allowed peer-disabled:opacity-70', 'Label');

export const Separator = React.forwardRef(({ className, orientation = 'horizontal', decorative = true, ...props }, ref) =>
  h('div', { ref, role: decorative ? 'none' : 'separator', 'aria-orientation': orientation, className: cn('shrink-0 bg-border', orientation === 'horizontal' ? 'h-[1px] w-full' : 'h-full w-[1px]', className), ...props }));
Separator.displayName = 'Separator';

export const Progress = React.forwardRef(({ className, value, max = 100, ...props }, ref) => {
  const pct = Math.max(0, Math.min(100, ((Number(value) || 0) / (max || 100)) * 100));
  return h('div', { ref, role: 'progressbar', 'aria-valuemin': 0, 'aria-valuemax': max, 'aria-valuenow': value, className: cn('relative h-4 w-full overflow-hidden rounded-full bg-secondary', className), ...props },
    h('div', { className: 'h-full bg-primary transition-all', style: { width: `${pct}%` } }));
});
Progress.displayName = 'Progress';

const ALERT = { default: 'bg-background text-foreground', destructive: 'border-destructive/50 text-destructive dark:border-destructive [&>svg]:text-destructive' };
export const Alert = React.forwardRef(({ className, variant = 'default', ...props }, ref) =>
  h('div', { ref, role: 'alert', className: cn('relative w-full rounded-lg border p-4 [&>svg~*]:pl-7 [&>svg+div]:translate-y-[-3px] [&>svg]:absolute [&>svg]:left-4 [&>svg]:top-4 [&>svg]:text-foreground', ALERT[variant] || ALERT.default, className), ...props }));
Alert.displayName = 'Alert';
export const AlertTitle = el('h5', 'mb-1 font-medium leading-none tracking-tight', 'AlertTitle');
export const AlertDescription = el('div', 'text-sm [&_p]:leading-relaxed', 'AlertDescription');

export const Checkbox = React.forwardRef(({ className, checked, defaultChecked, onCheckedChange, onChange, ...props }, ref) =>
  h('input', {
    ref, type: 'checkbox', checked, defaultChecked,
    className: cn('peer h-4 w-4 shrink-0 cursor-pointer accent-[hsl(var(--primary))] disabled:cursor-not-allowed disabled:opacity-50', className),
    onChange: (e) => { if (onChange) onChange(e); if (onCheckedChange) onCheckedChange(e.target.checked); },
    ...props,
  }));
Checkbox.displayName = 'Checkbox';

export const Switch = React.forwardRef(({ className, checked, defaultChecked = false, onCheckedChange, disabled, ...props }, ref) => {
  const [on, set] = useControlled(checked, defaultChecked, onCheckedChange);
  return h('button', {
    ref, type: 'button', role: 'switch', 'aria-checked': on, 'data-state': on ? 'checked' : 'unchecked', disabled,
    className: cn('peer inline-flex h-6 w-11 shrink-0 cursor-pointer items-center rounded-full border-2 border-transparent transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50', on ? 'bg-primary' : 'bg-input', className),
    onClick: () => set(!on),
    ...props,
  }, h('span', { className: cn('pointer-events-none block h-5 w-5 rounded-full bg-background shadow-lg ring-0 transition-transform', on ? 'translate-x-5' : 'translate-x-0') }));
});
Switch.displayName = 'Switch';

// one thumb, as most artifacts use it; value and onValueChange are arrays as in Radix
export const Slider = React.forwardRef(({ className, value, defaultValue, onValueChange, onValueCommit, min = 0, max = 100, step = 1, disabled, ...props }, ref) => {
  const [v, set] = useControlled(value, defaultValue || [min], onValueChange);
  return h('input', {
    ref, type: 'range', min, max, step, disabled, value: v[0],
    className: cn('w-full cursor-pointer accent-[hsl(var(--primary))] disabled:cursor-not-allowed disabled:opacity-50', className),
    onChange: (e) => set([Number(e.target.value)]),
    onPointerUp: (e) => { if (onValueCommit) onValueCommit([Number(e.target.value)]); },
    ...props,
  });
});
Slider.displayName = 'Slider';

const TabsCtx = React.createContext({ value: undefined, set: () => {} });

export const Tabs = React.forwardRef(({ value, defaultValue, onValueChange, ...props }, ref) => {
  const [v, set] = useControlled(value, defaultValue, onValueChange);
  return h(TabsCtx.Provider, { value: { value: v, set } }, h('div', { ref, ...props }));
});
Tabs.displayName = 'Tabs';
export const TabsList = React.forwardRef(({ className, ...props }, ref) =>
  h('div', { ref, role: 'tablist', className: cn('inline-flex h-10 items-center justify-center rounded-md bg-muted p-1 text-muted-foreground', className), ...props }));
TabsList.displayName = 'TabsList';
export const TabsTrigger = React.forwardRef(({ className, value, onClick, ...props }, ref) => {
  const ctx = React.useContext(TabsCtx);
  const on = ctx.value === value;
  return h('button', {
    ref, type: 'button', role: 'tab', 'aria-selected': on, 'data-state': on ? 'active' : 'inactive',
    className: cn('inline-flex items-center justify-center whitespace-nowrap rounded-sm px-3 py-1.5 text-sm font-medium ring-offset-background transition-all focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-50', on && 'bg-background text-foreground shadow-sm', className),
    onClick: chain(onClick, () => ctx.set(value)),
    ...props,
  });
});
TabsTrigger.displayName = 'TabsTrigger';
export const TabsContent = React.forwardRef(({ className, value, ...props }, ref) =>
  React.useContext(TabsCtx).value === value ? h('div', { ref, role: 'tabpanel', className: cn('mt-2 ring-offset-background focus-visible:outline-none', className), ...props }) : null);
TabsContent.displayName = 'TabsContent';

const DialogCtx = React.createContext({ open: false, set: () => {} });

export function Dialog({ open, defaultOpen = false, onOpenChange, children }) {
  const [o, set] = useControlled(open, defaultOpen, onOpenChange);
  return h(DialogCtx.Provider, { value: { open: o, set } }, children);
}
export function DialogTrigger({ asChild, onClick, ...props }) {
  const ctx = React.useContext(DialogCtx);
  return slot(asChild, 'button', { type: asChild ? undefined : 'button', onClick: chain(onClick, () => ctx.set(true)), ...props });
}
export function DialogClose({ asChild, onClick, ...props }) {
  const ctx = React.useContext(DialogCtx);
  return slot(asChild, 'button', { type: asChild ? undefined : 'button', onClick: chain(onClick, () => ctx.set(false)), ...props });
}
export const DialogContent = React.forwardRef(({ className, children, ...props }, ref) => {
  const ctx = React.useContext(DialogCtx);
  React.useEffect(() => {
    if (!ctx.open) return undefined;
    const key = (e) => { if (e.key === 'Escape') ctx.set(false); };
    document.addEventListener('keydown', key);
    return () => document.removeEventListener('keydown', key);
  }, [ctx.open]);
  if (!ctx.open) return null;
  return h('div', { className: 'fixed inset-0 z-50 flex items-center justify-center bg-black/80 p-4', onMouseDown: (e) => { if (e.target === e.currentTarget) ctx.set(false); } },
    h('div', { ref, role: 'dialog', 'aria-modal': true, className: cn('relative grid w-full max-w-lg gap-4 border bg-background p-6 shadow-lg sm:rounded-lg', className), ...props },
      children,
      h('button', { type: 'button', 'aria-label': 'Close', className: 'absolute right-4 top-4 rounded-sm opacity-70 transition-opacity hover:opacity-100', onClick: () => ctx.set(false) }, '✕')));
});
DialogContent.displayName = 'DialogContent';
export const DialogHeader = el('div', 'flex flex-col space-y-1.5 text-center sm:text-left', 'DialogHeader');
export const DialogFooter = el('div', 'flex flex-col-reverse sm:flex-row sm:justify-end sm:space-x-2', 'DialogFooter');
export const DialogTitle = el('h2', 'text-lg font-semibold leading-none tracking-tight', 'DialogTitle');
export const DialogDescription = el('p', 'text-sm text-muted-foreground', 'DialogDescription');

const SelectCtx = React.createContext(null);

// the [value, label] of every SelectItem written inside the Select, so the
// trigger can show the chosen one's label while the list is closed
function selectItems(children, out) {
  React.Children.forEach(children, (c) => {
    if (!React.isValidElement(c)) return;
    if (c.type === SelectItem) out.push([c.props.value, c.props.children]);
    else if (c.props && c.props.children) selectItems(c.props.children, out);
  });
  return out;
}

export function Select({ value, defaultValue, onValueChange, disabled, children }) {
  const [v, set] = useControlled(value, defaultValue, onValueChange);
  const [open, setOpen] = React.useState(false);
  const box = React.useRef(null);
  React.useEffect(() => {
    if (!open) return undefined;
    const away = (e) => { if (box.current && !box.current.contains(e.target)) setOpen(false); };
    document.addEventListener('mousedown', away);
    return () => document.removeEventListener('mousedown', away);
  }, [open]);
  const item = selectItems(children, []).find(([iv]) => iv === v);
  const ctx = { value: v, label: item && item[1], open, disabled, setOpen, pick: (x) => { set(x); setOpen(false); } };
  return h(SelectCtx.Provider, { value: ctx }, h('div', { ref: box, className: 'relative' }, children));
}
export const SelectTrigger = React.forwardRef(({ className, children, ...props }, ref) => {
  const ctx = React.useContext(SelectCtx);
  return h('button', {
    ref, type: 'button', 'aria-haspopup': 'listbox', 'aria-expanded': ctx.open, disabled: ctx.disabled,
    className: cn('flex h-10 w-full items-center justify-between rounded-md border border-input bg-background px-3 py-2 text-sm ring-offset-background focus:outline-none focus:ring-2 focus:ring-ring disabled:cursor-not-allowed disabled:opacity-50 [&>span]:line-clamp-1', className),
    onClick: () => ctx.setOpen(!ctx.open),
    ...props,
  }, children, h('span', { 'aria-hidden': true, className: 'ml-2 opacity-50' }, '▾'));
});
SelectTrigger.displayName = 'SelectTrigger';
export function SelectValue({ placeholder }) {
  const ctx = React.useContext(SelectCtx);
  const has = ctx.value !== undefined && ctx.value !== '';
  return h('span', { className: has ? '' : 'text-muted-foreground' }, has ? (ctx.label ?? ctx.value) : placeholder);
}
export const SelectContent = React.forwardRef(({ className, children, ...props }, ref) => {
  const ctx = React.useContext(SelectCtx);
  if (!ctx.open) return null;
  return h('div', { ref, role: 'listbox', className: cn('absolute left-0 z-50 mt-1 max-h-72 w-full min-w-[8rem] overflow-auto rounded-md border bg-popover p-1 text-popover-foreground shadow-md', className), ...props }, children);
});
SelectContent.displayName = 'SelectContent';
export const SelectItem = React.forwardRef(({ className, value, disabled, children, ...props }, ref) => {
  const ctx = React.useContext(SelectCtx);
  const on = ctx.value === value;
  return h('div', {
    ref, role: 'option', 'aria-selected': on, 'aria-disabled': disabled || undefined,
    className: cn('relative flex w-full cursor-default select-none items-center rounded-sm py-1.5 pl-8 pr-2 text-sm outline-none hover:bg-accent hover:text-accent-foreground', disabled && 'pointer-events-none opacity-50', className),
    onClick: () => { if (!disabled) ctx.pick(value); },
    ...props,
  }, on ? h('span', { className: 'absolute left-2 flex h-3.5 w-3.5 items-center justify-center' }, '✓') : null, children);
});
SelectItem.displayName = 'SelectItem';
export const SelectGroup = el('div', '', 'SelectGroup');
export const SelectLabel = el('div', 'py-1.5 pl-8 pr-2 text-sm font-semibold', 'SelectLabel');
export const SelectSeparator = el('div', '-mx-1 my-1 h-px bg-muted', 'SelectSeparator');

const TipCtx = React.createContext({ open: false });

export function TooltipProvider({ children }) {
  return h(React.Fragment, null, children);
}
export function Tooltip({ children }) {
  const [open, setOpen] = React.useState(false);
  return h(TipCtx.Provider, { value: { open } }, h('span', {
    className: 'relative inline-flex',
    onMouseEnter: () => setOpen(true), onMouseLeave: () => setOpen(false), onFocus: () => setOpen(true), onBlur: () => setOpen(false),
  }, children));
}
export function TooltipTrigger({ asChild, ...props }) {
  return slot(asChild, 'button', { type: asChild ? undefined : 'button', ...props });
}
export const TooltipContent = React.forwardRef(({ className, side = 'top', ...props }, ref) => {
  if (!React.useContext(TipCtx).open) return null;
  const at = side === 'bottom' ? 'top-full mt-2' : side === 'left' ? 'right-full mr-2 top-1/2 -translate-y-1/2' : side === 'right' ? 'left-full ml-2 top-1/2 -translate-y-1/2' : 'bottom-full mb-2';
  const across = side === 'top' || side === 'bottom' ? 'left-1/2 -translate-x-1/2' : '';
  return h('div', { ref, role: 'tooltip', className: cn('pointer-events-none absolute z-50 whitespace-nowrap rounded-md border bg-popover px-3 py-1.5 text-sm text-popover-foreground shadow-md', at, across, className), ...props });
});
TooltipContent.displayName = 'TooltipContent';
