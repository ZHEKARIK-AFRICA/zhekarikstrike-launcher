import { listen } from '@tauri-apps/api/event';

export function createPageListener({ listenFn = listen, target = window } = {}) {
    return async (eventName, handler) => {
        let pageHidden = false;
        let unlisten;

        target.addEventListener('pagehide', () => {
            pageHidden = true;
            if (unlisten) {
                unlisten();
                unlisten = undefined;
            }
        }, { once: true });

        unlisten = await listenFn(eventName, handler);
        if (pageHidden) {
            unlisten();
            unlisten = undefined;
        }
    };
}

export const listenUntilPageHide = createPageListener();
