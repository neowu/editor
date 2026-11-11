# Mermaid: core diagrams

Check each diagram renders (not an error box), text is legible in the
current theme, and switching theme (light ↔ dark) re-renders the colors.

## Flowchart

```mermaid
flowchart TD
    A[Start] --> B{Is it working?}
    B -- Yes --> C[Ship it]
    B -- No --> D[Debug]
    D --> E((Fix))
    E --> B
    C --> F[(Database)]
    subgraph Review
        G[/Read diff/] --> H[\Approve\]
    end
    F -.-> G
    H ==> I>Done]
```

## Flowchart (left to right, `graph` keyword)

```mermaid
graph LR
    client[Client] -->|HTTP| lb[Load balancer]
    lb --> s1[Server 1]
    lb --> s2[Server 2]
    s1 & s2 --> db[(Postgres)]
    classDef hot fill:#f96,stroke:#333
    class lb hot
```

## Sequence

```mermaid
sequenceDiagram
    autonumber
    actor User
    participant App
    participant API
    User->>App: Click "Save"
    activate App
    App->>API: PUT /doc
    alt success
        API-->>App: 200 OK
    else conflict
        API-->>App: 409 Conflict
        App->>User: Show merge dialog
    end
    loop every 30s
        App->>API: heartbeat
    end
    Note over App,API: All calls are authenticated
    deactivate App
```

## Class

```mermaid
classDiagram
    class Animal {
        +String name
        +int age
        +makeSound() void
    }
    class Dog {
        +fetch() void
    }
    class Cat {
        -bool indoor
        +purr() void
    }
    Animal <|-- Dog
    Animal <|-- Cat
    Dog "1" --> "*" Toy : plays with
    class Toy
```

## State

```mermaid
stateDiagram-v2
    [*] --> Idle
    Idle --> Loading : fetch
    Loading --> Ready : ok
    Loading --> Error : fail
    Error --> Loading : retry
    Ready --> [*]
    state Ready {
        [*] --> Viewing
        Viewing --> Editing : edit
        Editing --> Viewing : save
    }
```

## Entity relationship

```mermaid
erDiagram
    CUSTOMER ||--o{ ORDER : places
    ORDER ||--|{ LINE_ITEM : contains
    PRODUCT ||--o{ LINE_ITEM : "ordered in"
    CUSTOMER {
        string name
        string email PK
    }
    ORDER {
        int id PK
        date created
    }
```
