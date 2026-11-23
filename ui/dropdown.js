/* Shared desktop dropdown for single-choice controls and model shortcuts. */
function createDropdown(React, jsx) {
  return function Dropdown({label, value, items, onChange, trigger, align = 'start', minWidth = 160, empty}) {
    const id = React.useId();
    const button = React.useRef(null);
    const menu = React.useRef(null);
    const [open, setOpen] = React.useState(false);
    const choices = () => [...menu.current.querySelectorAll('[role^="menuitem"]')];
    const current = items.find(item => item.value === value && !item.action);

    React.useEffect(() => {
      const popup = menu.current;
      const sync = () => setOpen(popup.matches(':popover-open'));
      const dismiss = event => {
        const target = event.target;
        if (target instanceof Node && (popup.contains(target) || button.current?.contains(target))) return;
        popup.hidePopover();
      };
      popup.addEventListener('toggle', sync);
      document.addEventListener('pointerdown', dismiss, true);
      window.addEventListener('resize', dismiss);
      window.addEventListener('scroll', dismiss, true);
      return () => {
        popup.removeEventListener('toggle', sync);
        document.removeEventListener('pointerdown', dismiss, true);
        window.removeEventListener('resize', dismiss);
        window.removeEventListener('scroll', dismiss, true);
      };
    }, []);

    function show(last = false) {
      const popup = menu.current;
      const anchor = button.current.getBoundingClientRect();
      const width = Math.min(Math.max(anchor.width, minWidth), window.innerWidth - 16);
      const left = align === 'end' ? anchor.right - width :
        align === 'center' ? anchor.left + (anchor.width - width) / 2 : anchor.left;
      const below = window.innerHeight - anchor.bottom - 14;
      const above = anchor.top - 14;
      const upwards = below < Math.min(280, items.length * 42 + 20) && above > below;

      Object.assign(popup.style, {
        width: width + 'px',
        left: Math.max(8, Math.min(left, window.innerWidth - width - 8)) + 'px',
        maxHeight: Math.min(280, upwards ? above : below) + 'px',
        top: upwards ? 'auto' : anchor.bottom + 6 + 'px',
        bottom: upwards ? window.innerHeight - anchor.top + 6 + 'px' : 'auto',
      });
      popup.showPopover();
      const rows = choices();
      const selected = rows.find(row => row.getAttribute('aria-checked') === 'true');
      (last ? rows.at(-1) : selected || rows[0])?.focus();
    }

    function onMenuKey(event) {
      const {key} = event;
      if (key === 'Escape') {
        event.preventDefault();
        event.stopPropagation();
        menu.current.hidePopover();
        button.current.focus();
        return;
      }
      if (key === 'Tab') {
        menu.current.hidePopover();
        button.current.focus();
        return;
      }
      if (!['ArrowDown','ArrowUp','Home','End'].includes(key)) return;
      event.preventDefault();
      const rows = choices();
      if (!rows.length) return;
      const index = rows.indexOf(document.activeElement);
      const next = key === 'Home' ? 0 : key === 'End' ? rows.length - 1 :
        (index + (key === 'ArrowDown' ? 1 : -1) + rows.length) % rows.length;
      rows[next].focus();
    }

    return jsx('div', {className:'mono-dropdown', children:[
      jsx('button', {
        ref:button, type:'button', className:'mono-dropdown-trigger',
        'aria-haspopup':'menu', 'aria-expanded':open, 'aria-controls':id,
        'aria-label':current ? label + ' · ' + current.label : label,
        onClick:() => menu.current.matches(':popover-open') ? menu.current.hidePopover() : show(),
        onKeyDown:event => {
          if (!['ArrowDown','ArrowUp'].includes(event.key)) return;
          event.preventDefault();
          if (!menu.current.matches(':popover-open')) show(event.key === 'ArrowUp');
        },
        children:[trigger || jsx('span',{className:'mono-dropdown-label',children:current?.label || label}),
          jsx('span',{className:'mono-dropdown-chevron','aria-hidden':true})],
      }),
      jsx('div', {
        ref:menu, id, role:'menu', popover:'manual', 'aria-label':label,
        className:'mono-dropdown-menu', onKeyDown:onMenuKey,
        children:[
          empty && jsx('div',{className:'mono-dropdown-empty',children:empty}),
          ...items.map(item => {
            const selected = !item.action && item.value === value;
            return jsx('button',{
              type:'button', tabIndex:-1, role:item.action ? 'menuitem' : 'menuitemradio',
              'aria-checked':item.action ? undefined : selected,
              className:item.action ? 'mono-dropdown-option mono-dropdown-action' : 'mono-dropdown-option',
              onClick:() => {
                menu.current.hidePopover();
                button.current.focus({preventScroll:true});
                if (item.action) item.action();
                else onChange(item.value);
              },
              children:[
                jsx('span',{className:'mono-dropdown-content',children:[
                  jsx('span',{className:'mono-dropdown-label',children:item.label}),
                  item.detail && jsx('span',{className:'mono-dropdown-detail',children:item.detail}),
                ]}),
                !item.action && jsx('span',{className:'mono-dropdown-indicator','aria-hidden':true,children:selected ? '✓' : ''}),
              ],
            }, item.value);
          }),
        ],
      }),
    ]});
  };
}
