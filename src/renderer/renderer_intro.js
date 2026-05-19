// renderer_intro.js
document.addEventListener('DOMContentLoaded', () => {
    document.body.classList.add('fade-in');
});

function fadeOutAndNavigate(page) {
    console.log('fadeOutAndNavigate called with page:', page);
    document.body.classList.remove('fade-in');
    document.body.classList.add('fade-out');

    document.body.addEventListener('animationend', function handler() {
        document.body.removeEventListener('animationend', handler);
        window.electronAPI.navigateToPage(page);
    });
}

window.electronAPI.on('start-fade-out', (event, nextPage) => {
    fadeOutAndNavigate(nextPage);
});
