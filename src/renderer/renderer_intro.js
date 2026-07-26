import { listen } from '@tauri-apps/api/event';

import './e2e.js';
import { navigateToPage } from './navigation.js';

document.addEventListener('DOMContentLoaded', () => {
    document.body.classList.add('fade-in');
});

const unlisteners = [];
listen('start-fade-out', ({ payload: nextPage }) => {
    document.body.classList.remove('fade-in');
    document.body.classList.add('fade-out');
    document.body.addEventListener('animationend', async function handler() {
        document.body.removeEventListener('animationend', handler);
        await navigateToPage(nextPage);
    });
}).then((unlisten) => unlisteners.push(unlisten));

window.addEventListener('pagehide', () => {
    unlisteners.splice(0).forEach((unlisten) => unlisten());
});
