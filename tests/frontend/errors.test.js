import { describe, expect, it } from 'vitest';

import { errorMessage } from '../../src/renderer/errors.js';

describe('errorMessage', () => {
    it('preserves a string returned by a Tauri command', () => {
        expect(errorMessage('download failed')).toBe('download failed');
    });

    it('uses the public message from a structured AppError', () => {
        expect(errorMessage({
            code: 'INVALID_DATA',
            message: 'update rejected',
            details: 'signature mismatch'
        })).toBe('update rejected');
    });

    it('falls back to details when the public message is absent', () => {
        expect(errorMessage({ code: 'INVALID_DATA', details: 'signature mismatch' }))
            .toBe('signature mismatch');
    });
});
