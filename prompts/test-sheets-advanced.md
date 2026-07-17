In the Drive folder "GWS MCP Test", create a spreadsheet named "Sales Tracker" with the following:

1. Write a header row and sample data:
   - Headers: Rep, Region, Q1 Sales, Q2 Sales, Total, Status
   - Row 2: Alice, North, 45000, 52000, =C2+D2, (leave blank)
   - Row 3: Bob, South, 38000, 41000, =C3+D3, (leave blank)
   - Row 4: Carol, East, 51000, 48000, =C4+D4, (leave blank)
   - Row 5: Dave, West, 29000, 35000, =C5+D5, (leave blank)

2. Read back the data to verify the formulas calculated correctly

3. Add conditional formatting:
   - Highlight cells in the Total column (E2:E5) green when the value is greater than 80000
   - Highlight cells in the Total column (E2:E5) red when the value is less than 70000

4. Set data validation on the Status column (F2:F5):
   - Allow only: "On Track", "At Risk", "Behind"
   - Show a dropdown

5. Create a named range called "SalesData" covering A1:F5

6. Read back the spreadsheet info to verify the tab structure

7. Insert 2 new rows after the existing data (at row 6)

8. Export the spreadsheet as CSV

9. List all formulas in the spreadsheet and explain the formula in cell E2

10. Write values into the Status column: "On Track", "At Risk", "On Track", "Behind"

11. Read the final data to verify everything is correct
