/**
 * Epub-rs Frontend DOM to CFI Mapper
 * 
 * This script demonstrates how to map clicks or text selections in the browser
 * back to the EPUB CFI format, utilizing the `data-cfi` attributes injected 
 * by the `epub-rs` backend parser.
 */

// ==========================================
// 1. Get CFI from a clicked element
// ==========================================
document.addEventListener('click', (e) => {
    // Find the closest element with a data-cfi attribute
    const cfiElement = e.target.closest('[data-cfi]');
    if (cfiElement) {
        const baseCfi = cfiElement.getAttribute('data-cfi');
        console.log("Clicked Element CFI:", baseCfi);
        // Example Output: epubcfi(/6/6/4[chapter1]!/4/2[wrapper]/4[p2])
    }
});

// ==========================================
// 2. Generate CFI for a user's text selection (Highlighting)
// ==========================================
function getSelectionCfi() {
    const selection = window.getSelection();
    if (!selection || selection.rangeCount === 0) return null;

    const range = selection.getRangeAt(0);
    
    // Get the start element that holds the text node
    let startElement = range.startContainer;
    if (startElement.nodeType === Node.TEXT_NODE) {
        startElement = startElement.parentElement;
    }
    
    // Find the nearest injected base CFI path
    const cfiContainer = startElement.closest('[data-cfi]');
    if (!cfiContainer) return null;

    const startCfiBase = cfiContainer.getAttribute('data-cfi');
    
    // Calculate character offset within the text node.
    // In CFI, text nodes are odd numbers. The first text child of an element is usually /1.
    const startOffset = range.startOffset;
    
    // Remove the closing ')' from the base CFI and append the text node offset.
    // e.g., epubcfi(/6/6/4!/4) -> epubcfi(/6/6/4!/4/1:15)
    // Note: A strict CFI parser would also count preceding text nodes to find if it's /1, /3, etc.
    // For simplicity, we assume the first text child /1.
    const exactCfi = startCfiBase.replace(')', `/1:${startOffset})`);
    
    return exactCfi;
}

// ==========================================
// 3. Jump to a CFI (Restore bookmark)
// ==========================================
function jumpToCfi(cfiString) {
    // Extract the path up to the last element (ignoring text node /1:xxx or assertions like [id])
    // A simple regex to find the matching data-cfi base path
    // e.g., extract "/6/6/4!/4/2" from "epubcfi(/6/6/4!/4/2/1:15)"
    const match = cfiString.match(/epubcfi\((.*?)(?:\/\d+:\d+)?\)/);
    if (match && match[1]) {
        // Find the element with this exact data-cfi string
        const searchCfi = `epubcfi(${match[1]})`;
        const targetElement = document.querySelector(`[data-cfi="${searchCfi}"]`);
        
        if (targetElement) {
            targetElement.scrollIntoView({ behavior: 'smooth', block: 'center' });
            
            // Optional: Highlight it to show where the bookmark is
            targetElement.style.backgroundColor = 'rgba(255, 255, 0, 0.3)';
            setTimeout(() => targetElement.style.backgroundColor = '', 2000);
            
            return true;
        }
    }
    console.warn("CFI target not found in the current DOM.");
    return false;
}
