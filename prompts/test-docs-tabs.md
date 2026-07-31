Find the Drive folder named "GWS MCP Test" and create a Google Doc named "Tabs Test Report" in it.

Write some content into the document — a heading "Overview" and a short paragraph about document tabs.

After creating the document:
1. Use gws_docs_read with list_tabs=true to list the document's tabs — report how many tabs there are and their titles
2. Use gws_docs_read with the tab parameter set to the title of the first tab to read its content
3. Use gws_docs_read with the tab parameter set to a tab title that does NOT exist (e.g. "Nonexistent Tab") — report the error message you get back
4. Use gws_docs_outline with the tab parameter set to the first tab's title to get the outline
5. Use gws_docs_find with text="tabs" and the tab parameter set to the first tab's title — report the character indexes
