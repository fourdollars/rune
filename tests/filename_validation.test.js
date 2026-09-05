// Unit test for Unicode filename validation and editor autosave logic
const assert = require('assert');

console.log('=== Rune Notes Filename Validation Test Suite ===');

// Test 1: Frontend regex validates ASCII and Unicode/Chinese filenames
{
  const filenameRegex = /^[\p{L}\p{N}_\-\.]+\.md$/u;

  // Valid filenames
  assert.ok(filenameRegex.test('01-數與式.md'), 'Chinese filename 01-數與式.md should be valid');
  assert.ok(filenameRegex.test('高一數學.md'), 'Chinese note/file 高一數學.md should be valid');
  assert.ok(filenameRegex.test('spec.md'), 'ASCII filename spec.md should be valid');
  assert.ok(filenameRegex.test('my-doc.md'), 'Hyphenated filename my-doc.md should be valid');
  assert.ok(filenameRegex.test('arch_v2.md'), 'Underscored filename arch_v2.md should be valid');
  assert.ok(filenameRegex.test('CAPS.md'), 'Uppercase filename CAPS.md should be valid');
  assert.ok(filenameRegex.test('résumé_v1.md'), 'Accented Latin résumé_v1.md should be valid');
  assert.ok(filenameRegex.test('日本語ノート.md'), 'Japanese filename 日本語ノート.md should be valid');

  // Invalid filenames
  assert.ok(!filenameRegex.test(''), 'Empty string should be invalid');
  assert.ok(!filenameRegex.test('file.txt'), 'Non-md extension should be invalid');
  assert.ok(!filenameRegex.test('../etc/passwd.md'), 'Path traversal ../ should be invalid');
  assert.ok(!filenameRegex.test('file name.md'), 'Filenames with spaces should be invalid');
  assert.ok(!filenameRegex.test('file;rm.md'), 'Filenames with semicolon should be invalid');
  assert.ok(!filenameRegex.test('file/name.md'), 'Filenames with slash should be invalid');
  assert.ok(!filenameRegex.test('file\\name.md'), 'Filenames with backslash should be invalid');

  console.log('✓ Test 1 passed: Unicode and ASCII filenames validated properly by frontend regex');
}

// Test 2: Auto-save condition guards against empty currentFilename
{
  function shouldAutoSave(editorDirty, currentNoteId, currentFilename) {
    return Boolean(editorDirty && currentNoteId && currentFilename);
  }

  // When visiting a note with no file selected / empty filename
  assert.strictEqual(
    shouldAutoSave(true, '高一數學', ''),
    false,
    'Should NOT auto-save when currentFilename is empty string'
  );

  assert.strictEqual(
    shouldAutoSave(true, '高一數學', null),
    false,
    'Should NOT auto-save when currentFilename is null'
  );

  assert.strictEqual(
    shouldAutoSave(true, '', '01-數與式.md'),
    false,
    'Should NOT auto-save when currentNoteId is empty'
  );

  assert.strictEqual(
    shouldAutoSave(false, '高一數學', '01-數與式.md'),
    false,
    'Should NOT auto-save when editor is not dirty'
  );

  assert.strictEqual(
    shouldAutoSave(true, '高一數學', '01-數與式.md'),
    true,
    'Should auto-save when editor is dirty, currentNoteId and currentFilename are valid'
  );

  console.log('✓ Test 2 passed: Auto-save guard correctly rejects empty or missing filename');
}

console.log('All filename validation tests passed successfully! 🎉');
