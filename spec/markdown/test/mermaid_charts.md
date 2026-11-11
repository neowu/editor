# Mermaid: charts and hierarchies

Check each renders, labels don't overlap badly, and colors follow the theme.

## Railroad

```mermaid
railroad-beta
title Expression grammar

expression = sequence(
    nonterminal("term"),
    zeroOrMore(sequence(
        choice(terminal("+"), terminal("-")),
        nonterminal("term")
    ))
) ;
term = oneOrMore(nonterminal("digit")) ;
```

## XY chart

```mermaid
xychart-beta
    title "Sales revenue"
    x-axis [jan, feb, mar, apr, may, jun]
    y-axis "Revenue (k$)" 0 --> 100
    bar [52, 60, 75, 82, 70, 95]
    line [52, 60, 75, 82, 70, 95]
```

## Treemap

```mermaid
treemap-beta
"Budget"
    "Operations"
        "Salaries": 700000
        "Equipment": 200000
    "Marketing"
        "Advertising": 400000
        "Events": 100000
```

## Timeline

```mermaid
timeline
    title History of the editor
    2023 : Fork created
    2024 : Trimmed panels
         : Custom keymap
    2025 : Own markdown renderer
    2026 : Owned Mermaid crate
```

## Mindmap

```mermaid
mindmap
  root((Editor))
    Markdown
      Preview
      HTML subset
    Diagrams
      Flowchart
      Sequence
    Git
      Blame
      Diff
```
