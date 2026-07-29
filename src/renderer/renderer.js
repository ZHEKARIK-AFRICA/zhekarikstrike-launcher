import { invoke } from '@tauri-apps/api/core';
import { getCurrentWindow } from '@tauri-apps/api/window';

import { t } from '../localization/i18n.js';
import { setupInputContextMenu } from './context-menu.js';
import { listenUntilPageHide } from './event-listener.js';

let closeRequest = null;
let closeActionPending = false;

function renderCloseConfirmation() {
    if (!closeRequest) return;
    const gameRunning = closeRequest.reason === 'game-running';
    const modal = document.getElementById('close-confirmation-modal');
    const title = document.getElementById('close-confirmation-title');
    const message = document.getElementById('close-confirmation-message');
    const cancel = document.getElementById('close-confirmation-cancel');
    const confirm = document.getElementById('close-confirmation-confirm');
    if (title) title.textContent = t('close_confirmation.title');
    if (message) {
        message.textContent = t(gameRunning
            ? 'close_confirmation.game_running'
            : 'close_confirmation.operation_active');
    }
    if (cancel) cancel.textContent = t('close_confirmation.stay');
    if (confirm) {
        confirm.textContent = t(gameRunning
            ? 'close_confirmation.close_game_and_launcher'
            : 'close_confirmation.close_launcher');
    }
    if (modal) modal.style.display = 'block';
}

async function cancelClose() {
    if (closeActionPending) return;
    closeActionPending = true;
    try {
        await invoke('cancel_close_window');
        const modal = document.getElementById('close-confirmation-modal');
        if (modal) modal.style.display = 'none';
        closeRequest = null;
    } catch (error) {
        console.error('Failed to cancel window close:', error);
    } finally {
        closeActionPending = false;
    }
}

async function confirmClose() {
    if (closeActionPending) return;
    closeActionPending = true;
    document.getElementById('close-confirmation-cancel')?.setAttribute('disabled', '');
    document.getElementById('close-confirmation-confirm')?.setAttribute('disabled', '');
    window.dispatchEvent(new CustomEvent('launcher-closing'));
    try {
        await invoke('confirm_close_window');
    } catch (error) {
        closeActionPending = false;
        document.getElementById('close-confirmation-cancel')?.removeAttribute('disabled');
        document.getElementById('close-confirmation-confirm')?.removeAttribute('disabled');
        console.error('Failed to confirm window close:', error);
    }
}

void listenUntilPageHide('close-confirmation-requested', ({ payload }) => {
    closeRequest = payload;
    closeActionPending = false;
    renderCloseConfirmation();
});

window.addEventListener('language-changed', renderCloseConfirmation);
window.addEventListener('pagehide', () => {
    window.removeEventListener('language-changed', renderCloseConfirmation);
}, { once: true });

async function runWindowAction(action, label) {
    try {
        await action();
    } catch (error) {
        console.error(`Failed to ${label} window:`, error);
    }
}

document.addEventListener('DOMContentLoaded', () => {
    setupInputContextMenu();

    document.addEventListener('click', async (event) => {
        const target = event.target.closest('a');
        if (!target?.href || !/^https?:\/\//.test(target.href)) return;
        event.preventDefault();
        try {
            await invoke('open_external_url', { url: target.href });
        } catch (error) {
            console.error('Failed to open external link:', error);
        }
    });

    document.getElementById('close-window')?.addEventListener('click', () => {
        void runWindowAction(() => invoke('close_window'), 'close');
    });
    document.getElementById('minimize-window')?.addEventListener('click', () => {
        void runWindowAction(() => getCurrentWindow().minimize(), 'minimize');
    });
    document.getElementById('close-confirmation-cancel')?.addEventListener('click', () => {
        void cancelClose();
    });
    document.getElementById('close-confirmation-confirm')?.addEventListener('click', () => {
        void confirmClose();
    });

    const settingsButton = document.getElementById('settings-button');
    const settingsModal = document.getElementById('settings-modal');
    const closeSettingsButton = document.getElementById('close-settings');
    settingsButton?.addEventListener('click', () => { settingsModal.style.display = 'block'; });
    closeSettingsButton?.addEventListener('click', () => { settingsModal.style.display = 'none'; });
    window.addEventListener('click', (event) => {
        if (event.target === settingsModal) settingsModal.style.display = 'none';
    });
}, { once: true });
