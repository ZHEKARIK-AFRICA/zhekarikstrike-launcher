function contextButton(label, action) {
    const button = document.createElement('button');
    button.type = 'button';
    button.textContent = label;
    Object.assign(button.style, {
        display: 'block', width: '100%', padding: '5px 14px', color: '#fff',
        background: 'transparent', border: '0', textAlign: 'left', cursor: 'default'
    });
    button.addEventListener('click', async (event) => {
        event.stopPropagation();
        await action();
        button.parentElement.style.display = 'none';
    });
    return button;
}

export function setupInputContextMenu() {
    let menu = document.getElementById('tauri-input-context-menu');
    if (menu) return;

    menu = document.createElement('div');
    menu.id = 'tauri-input-context-menu';
    Object.assign(menu.style, {
        position: 'fixed', display: 'none', zIndex: '10000', background: '#1f1f1f',
        border: '1px solid #444', color: '#fff', fontSize: '12px', padding: '4px',
        boxShadow: '0 6px 18px rgba(0,0,0,.35)'
    });
    document.body.appendChild(menu);

    document.addEventListener('click', () => { menu.style.display = 'none'; });
    document.addEventListener('contextmenu', (event) => {
        const target = event.target;
        const editable = target instanceof HTMLInputElement
            || target instanceof HTMLTextAreaElement
            || target?.isContentEditable;
        if (!editable) return;

        event.preventDefault();
        menu.replaceChildren(
            contextButton('Cut', () => document.execCommand('cut')),
            contextButton('Copy', () => document.execCommand('copy')),
            contextButton('Paste', async () => {
                const text = await navigator.clipboard?.readText?.();
                if (text) document.execCommand('insertText', false, text);
            }),
            contextButton('Select All', () => {
                if (typeof target.select === 'function') target.select();
                else document.execCommand('selectAll');
            })
        );
        menu.style.left = `${event.clientX}px`;
        menu.style.top = `${event.clientY}px`;
        menu.style.display = 'block';
    });
}
