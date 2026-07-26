import { getLanguage, initializeLanguage, setLanguage, t } from './i18n.js';
import { waitForE2eReady } from '../renderer/e2e.js';

function setText(selector, key) {
    const element = document.querySelector(selector);
    if (element) element.textContent = t(key);
}

function applyTranslations() {
    setText('#zhekarik-strike-title', 'zhekarik_strike_title');
    setText('#page-title', 'install_game');
    setText('label[for="install-path"]', 'install_path');
    const installPath = document.getElementById('install-path');
    if (installPath) installPath.placeholder = t('install_placeholder');
    setText('#start-install', 'start_install');
    setText('#cancel-install', 'cancel_install');
    setText('#install-status', 'install_status');
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
