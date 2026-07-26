import './e2e.js';
import { listenUntilPageHide } from './event-listener.js';
import { navigateToPage } from './navigation.js';

document.addEventListener('DOMContentLoaded', () => {
    document.body.classList.add('fade-in');
});

listenUntilPageHide('start-fade-out', ({ payload: nextPage }) => {
    document.body.classList.remove('fade-in');
    document.body.classList.add('fade-out');
    document.body.addEventListener('animationend', async function handler() {
        document.body.removeEventListener('animationend', handler);
        await navigateToPage(nextPage);
    });
});
