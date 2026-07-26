import { getLanguage, initializeLanguage, setLanguage, t } from './i18n.js';
import { waitForE2eReady } from '../renderer/e2e.js';

function setText(selector, key) {
    const element = document.querySelector(selector);
    if (element) element.textContent = t(key);
}

function applyTranslations() {
    setText('#settings-modal h2', 'settings');
    setText('#zhekarik-strike-title', 'zhekarik_strike_title');
    setText('label[for="game-path"]', 'game_path');
    setText('#close-settings', 'close');
    setText('#play-button', 'play');
    setText('label[for="launch-params"]', 'launch_params');
    setText('label[for="clan-tag"]', 'clan_tag');
    setText('label[for="nickname"]', 'nickname');
    setText('#check-files', 'check_files');
    setText('#launcher-status', 'status_ready');
    setText('#error-message', 'error_message');
    setText('#error-modal-title', 'error_title');
    setText('#error-modal-ok', 'error_modal_ok');
    setText('a[href="https://zhekarik.africa/strike/stream"]', 'links.stream');
    setText('a[href="https://zhekarik.africa/strike/vip"]', 'links.vip');
    setText('a[href="https://zhekarik.africa/strike/demos"]', 'links.demos');
}

function updateActiveLanguage(language) {
    document.querySelectorAll('.language-option').forEach((option) => option.classList.remove('active'));
    document.getElementById(`language-${language}`)?.classList.add('active');
}

async function changeLanguage(language) {
    await setLanguage(language);
    applyTranslations();
    updateActiveLanguage(language);
}

document.addEventListener('DOMContentLoaded', async () => {
    await waitForE2eReady();
    await initializeLanguage();
    applyTranslations();
    updateActiveLanguage(getLanguage());
    document.getElementById('language-en')?.addEventListener('click', () => changeLanguage('en'));
    document.getElementById('language-ru')?.addEventListener('click', () => changeLanguage('ru'));
});
