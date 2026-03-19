# TeaLang Grammar Specification

## Overview

TeaLang is a statically-typed programming language with syntax inspired by Rust. Each program consists of `use` statements, variable declarations, structure definitions, and function declarations/definitions, which can appear in any order.

```
program := (useStmt | varDeclStmt | structDef | fnDeclStmt | fnDef)*
```

---

## Basic Elements

### Keywords

TeaLang reserves the following keywords:
- **`let`** - variable declaration
- **`fn`** - function declaration/definition
- **`struct`** - structure definition
- **`if`** / **`else`** - conditional branching
- **`while`** - loop construct
- **`break`** / **`continue`** - loop control
- **`return`** - function return
- **`i32`** - 32-bit integer type
- **`use`** - module import

### Identifiers

Identifiers begin with a letter or underscore, followed by any combination of letters, digits, or underscores. Identifiers cannot be keywords.

```
identifier := [a-zA-Z_][a-zA-Z0-9_]*
```

Examples: `x`, `count`, `my_getint`, `quickread`, `arr_1`

### Numeric Literals

TeaLang supports decimal integer literals. A number is either zero or starts with a non-zero digit.

```
num := 0 | [1-9][0-9]*
```

Examples: `0`, `1`, `42`, `1005`

### Whitespace

Spaces, tabs, newlines, and carriage returns are automatically skipped between tokens.

### Comments

TeaLang supports single-line and multi-line comments, which are automatically skipped between tokens.

- **Single-line comments**: start with `//` and continue to the end of the line.
- **Multi-line comments**: enclosed between `/*` and `*/`.

Examples:
```rust
// This is a single-line comment
/* This is a
   multi-line comment */
```

---

## Module System

### Use Statement

Import external modules using the `use` keyword. Module paths support multiple levels separated by `::`. `use` statements may appear anywhere at the top level, interleaved with other declarations.

```
modulePath := identifier (< :: > identifier)*

useStmt := < use > modulePath < ; >
```

Examples:
```rust
use std;               // single-level module
use a::b::c;           // multi-level module path
```

---

## Type System

### Type Specifications

TeaLang supports primitive types, user-defined types, and reference types.

```
refType  := < & > < [ > typeSpec < ] >
typeSpec := refType | < i32 > | identifier
```

Examples: `i32`, `Node`, `&[i32]`

### Variable Declarations

Variable declarations are categorized into four forms:

```
scalar_decl       := identifier                                             // scalar without type
typed_scalar_decl := identifier < : > typeSpec                             // scalar with type
array_decl        := identifier < [ > num < ] >                            // array without type
typed_array_decl  := identifier < : > < [ > typeSpec < ; > num < ] >       // array with type and size
```

```
varDecl := typed_array_decl
         | typed_scalar_decl
         | array_decl
         | scalar_decl
```

Since `typeSpec` includes reference types (`&[T]`), a `typed_scalar_decl` like `arr: &[i32]` declares a slice reference parameter. This form is only valid in function parameter lists — it cannot appear in `let` statements or struct fields. This constraint is enforced semantically, not syntactically.

Examples:
```rust
n:i32                    // typed_scalar_decl
count                    // scalar_decl
arr: [i32; 100]         // typed_array_decl
que[1005]               // array_decl
buf: &[i32]             // typed_scalar_decl with reference type (parameter only)
```

### Variable Declaration Statements

Declare variables with the `let` keyword, optionally initializing them. Array initializers support two forms: an explicit element list `[v1, v2, ...]` and a fill syntax `[val; n]` meaning `n` copies of `val`.

```
varDeclStmt := < let > varDef < ; >
             | < let > varDecl < ; >

varDef := typed_array_decl  < = > arrayInitializer   // typed array with initializer
        | typed_scalar_decl < = > rightVal            // typed scalar with initializer
        | array_decl        < = > arrayInitializer    // untyped array with initializer
        | scalar_decl       < = > rightVal            // scalar with initializer (type inferred)

arrayInitializer := < [ > rightValList < ] >          // explicit list: [1, 2, 3]
                  | < [ > rightVal < ; > num < ] >    // fill syntax:   [0; 5] means five 0s
```

> **Note:** Reference-typed declarations (`name: &[type]`) cannot appear in `let` statements.

Examples:
```rust
let n:i32;                              // declare typed scalar
let x:i32 = 0;                          // declare and initialize typed scalar
let arr: [i32; 3] = [1, 2, 3];         // declare and initialize typed array (explicit list)
let buf: [i32; 5] = [0; 5];            // declare and initialize typed array (fill syntax)
let que[3] = [1, 2, 3];               // declare and initialize untyped array (explicit list)
let que[5] = [0; 5];                  // declare and initialize untyped array (fill syntax)
let count = 0;                          // type inference scalar
```

---

## Structure Definitions

Define custom types using the `struct` keyword with named fields. Struct fields use `varDecl` (scalars and arrays only; reference types are not permitted as struct fields).

```
structDef := < struct > identifier < { > varDeclList < } >
varDeclList := varDecl (< , > varDecl)*
```

Example:
```rust
struct Node {
    value:i32,
    next:i32
}
```

---

## Functions

### Function Declarations

Declare function signatures with optional return types. Function parameters use `varDecl`, which includes reference-typed parameters like `arr: &[i32]` via `typeSpec`.

```
fnDeclStmt := fnDecl < ; >
fnDecl := < fn > identifier < ( > paramDecl? < ) > < -> > typeSpec   // with return type
        | < fn > identifier < ( > paramDecl? < ) >                   // without return type
paramDecl := varDeclList
```

Examples:
```rust
fn quickread() -> i32;                      // declaration with return type
fn move(x:i32, y:i32);                     // declaration without return type
fn init();                                  // no parameters
fn sum(arr: &[i32], n:i32) -> i32;         // reference parameter for array passing
```

### Function Definitions

Provide implementation by adding a code block to the declaration.

```
fnDef := fnDecl < { > codeBlockStmt* < } >
```

Example:
```rust
fn add(x:i32, y:i32) -> i32 {
    return x + y;
}

fn fill(arr: &[i32], n:i32) {
    let i:i32 = 0;
    while i < n {
        arr[i] = i;
        i = i + 1;
    }
}

fn main() -> i32 {
    let result:i32 = add(5, 3);
    return result;
}
```

### Function Calls

Functions can be called with module prefixes (for external functions) or locally. Array arguments are passed by reference using `&identifier`, which is a regular expression unit (see [Expression Units](#expression-units)). Module prefixes support multiple levels.

```
fnCall := modulePrefixedCall | localCall
modulePrefixedCall := modulePath < :: > identifier < ( > rightValList? < ) >
localCall := identifier < ( > rightValList? < ) >
```

Examples:
```rust
std::getint()               // single-level module prefix
a::b::getint()              // multi-level module prefix
quickread()                 // local function
addedge(x, y)              // local function with scalar arguments
fill(&arr, n)              // pass array by reference
std::putch(10)             // standard library with argument
```

---

## Statements

### Code Block Statements

Statements that can appear within function bodies:

```
codeBlockStmt := varDeclStmt
               | assignmentStmt
               | callStmt
               | ifStmt
               | whileStmt
               | returnStmt
               | continueStmt
               | breakStmt
               | nullStmt
```

### Assignment Statement

Assign values to variables, array elements, or structure fields.

```
assignmentStmt := leftVal < = > rightVal < ; >
leftVal := identifier leftValSuffix*
leftValSuffix := < [ > indexExpr < ] >                                 // array indexing
               | < . > identifier                                       // member access
indexExpr := num | identifier
```

Examples:
```rust
x = 5;                      // simple assignment
arr[i] = 10;               // array element assignment
node.value = x;            // struct field assignment
tail[i].next = head;       // chained access
```

### Call Statement

Execute a function and discard its return value.

```
callStmt := fnCall < ; >
```

Examples:
```rust
init();
std::putch(10);
addedge(x, y);
fill(&arr, n);
```

### Return Statement

Exit a function with or without a return value.

```
returnStmt := < return > rightVal < ; >
            | < return > < ; >
```

Examples:
```rust
return 0;
return x + y;
return;                     // void return
```

### If Statement

Conditional branching with optional else clause.

```
ifStmt := < if > boolExpr < { > codeBlockStmt* < } > < else > < { > codeBlockStmt* < } >
        | < if > boolExpr < { > codeBlockStmt* < } >
```

Example:
```rust
if x > 0 {
    return x;
}

if ch == 45 {
    f = 1;
} else {
    f = 0;
}
```

### While Statement

Loop with a boolean condition.

```
whileStmt := < while > boolExpr < { > codeBlockStmt* < } >
```

Example:
```rust
while i < n {
    i = i + 1;
}

while (ch >= 48) && (ch <= 57) {
    x = x * 10 + ch - 48;
    ch = std::getch();
}
```

### Break Statement

Exit from the innermost loop.

```
breakStmt := < break > < ; >
```

Example:
```rust
while 1 > 0 {
    if done {
        break;
    }
}
```

### Continue Statement

Skip to the next iteration of the loop.

```
continueStmt := < continue > < ; >
```

Example:
```rust
while i < n {
    if inq[temp] == 0 {
        continue;
    }
    i = i + 1;
}
```

### Null Statement

An empty statement (just a semicolon).

```
nullStmt := < ; >
```

---

## Expressions

### Right Values

Values that can appear on the right side of assignments.

```
rightVal := boolExpr | arithExpr
rightValList := rightVal (< , > rightVal)*
```

### Arithmetic Expressions

Arithmetic expressions support addition, subtraction, multiplication, and division with standard precedence.

```
arithExpr := arithTerm (arithAddOp arithTerm)*
arithTerm := exprUnit (arithMulOp exprUnit)*
arithAddOp := < + > | < - >
arithMulOp := < * > | < / >
```

Examples:
```rust
x + 1
n - 1
x * 10 + ch - 48
num / base
```

### Expression Units

Primary expressions that form the building blocks of larger expressions.

```
exprUnit := < ( > arithExpr < ) >
          | fnCall
          | < & > identifier                                // address-of: produces &[T] reference
          | < - > num                                       // negative literal
          | num
          | identifier exprSuffix*
exprSuffix := < [ > indexExpr < ] >                        // array indexing
            | < . > identifier                              // member access
```

Examples:
```rust
42
x
arr[i]
node.value
std::getint()
&arr                    // address-of (array reference)
-1                      // negative literal
(a + b) * c            // parenthesized expression
list[cnt].next         // chained access
```

### Boolean Expressions

Boolean expressions support logical AND, OR, NOT, and comparison operators.

```
boolExpr := boolAndTerm (< || > boolAndTerm)*
boolAndTerm := boolUnitAtom (< && > boolUnitAtom)*
boolUnitAtom := boolUnitParen
              | boolComparison
              | < ! > boolUnitAtom
boolUnitParen := < ( > boolExpr < ) >
               | < ( > exprUnit compOp exprUnit < ) >
boolComparison := exprUnit compOp exprUnit
compOp := < <= > | < >= > | < == > | < != > | < < > | < > >
```

Examples:
```rust
x > 0
x == 1
i != -1
(x >= 48) && (x <= 57)
(ch < 48) || (ch > 57)
!done
```

---

## Operators

### Arithmetic Operators

| Operator | Description       | Example |
|----------|-------------------|---------|
| `+`      | Addition          | `x + 1` |
| `-`      | Subtraction       | `n - 1` |
| `*`      | Multiplication    | `x * 10`|
| `/`      | Division          | `n / 2` |

### Comparison Operators

| Operator | Description              | Example  |
|----------|--------------------------|----------|
| `==`     | Equal to                 | `x == 1` |
| `!=`     | Not equal to             | `i != -1`|
| `<`      | Less than                | `i < n`  |
| `>`      | Greater than             | `a > max`|
| `<=`     | Less than or equal to    | `ch <= 57`|
| `>=`     | Greater than or equal to | `ch >= 48`|

### Logical Operators

| Operator | Description    | Example              |
|----------|----------------|----------------------|
| `&&`     | Logical AND    | `(x >= 0) && (x < 10)`|
| `\|\|`     | Logical OR     | `(ch < 48) \|\| (ch > 57)`|
| `!`      | Logical NOT    | `!done`              |

### Other Operators

| Operator | Description          | Example              |
|----------|----------------------|----------------------|
| `=`      | Assignment           | `x = 5;`             |
| `->`     | Function return type | `fn main() -> i32`   |
| `::`     | Module separator     | `std::getint()`      |
| `&`      | Reference / address-of | `fill(&arr, n)`    |

---

## Complete Example

```rust
use std;

struct Node {
    value:i32,
    next:i32
}

let head:i32;
let nodes: [Node; 100];
let count:i32 = 0;

fn init() {
    head = 0-1;
    count = 0;
}

fn add_node(val:i32) {
    nodes[count].value = val;
    nodes[count].next = head;
    head = count;
    count = count + 1;
}

fn fill(arr: &[i32], n:i32) {
    let i:i32 = 0;
    while i < n {
        arr[i] = std::getint();
        i = i + 1;
    }
}

fn main() -> i32 {
    init();

    let n:i32 = std::getint();
    let buf: [i32; 100];
    fill(&buf, n);

    let i:i32 = 0;
    while i < n {
        add_node(buf[i]);
        i = i + 1;
    }

    return 0;
}
```

---

## Notes

1. **Top-level Order**: `use` statements, variable declarations, struct definitions, and function declarations/definitions may appear in any order at the top level.
2. **Type Annotations**: Type annotations are optional for scalars and arrays in `let` statements but recommended for clarity.
3. **Array Initializers**: Two forms are supported:
   - `[val1, val2, ...]` — explicit element list (e.g., `[1, 2, 3]`)
   - `[val; n]` — fill syntax, equivalent to `n` copies of `val` (e.g., `[0; 5]` means five zeros)
4. **Reference Types**: Reference-typed declarations (`name: &[type]`) are only valid as function parameters. They cannot appear in `let` statements or struct fields. This constraint is enforced semantically, not syntactically.
5. **Passing Arrays by Reference**: Use `&identifier` at the call site to pass an array by reference: `fill(&arr, n)`. The `&identifier` form is a regular expression unit and the corresponding parameter must be declared with a reference type (e.g., `arr: &[i32]`).
6. **Variable Declaration Forms**:
   - `scalar_decl` — `name` (no type)
   - `typed_scalar_decl` — `name: type` (includes `name: &[type]` for reference parameters)
   - `array_decl` — `name[size]` (no type)
   - `typed_array_decl` — `name: [type; size]`
7. **Module System**: Module paths support multiple levels separated by `::` (e.g., `use a::b::c;`). Functions from external modules are called using the full module path as prefix (e.g., `a::b::fn_name(...)`).
8. **No Implicit Conversions**: All type conversions must be explicit.
9. **Operator Precedence**: Standard mathematical precedence applies (multiplication/division before addition/subtraction).
10. **Chained Access**: Array indexing and member access can be chained: `arr[i].field[j]`.
11. **Index Expressions**: Array indices accept only numeric literals or plain identifiers — not arbitrary arithmetic. To use a computed index, store it in a variable first: `let idx:i32 = i + 1; arr[idx] = val;`.
12. **Negation**: Only numeric literals can be directly negated (`-1`, `-42`). To negate a variable or expression, use subtraction from zero: `0 - x`.
