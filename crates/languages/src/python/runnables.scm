; pytest functions
((module
  (function_definition
    name: (identifier) @run @_pytest_method_name
    (#match? @_pytest_method_name "^test_")) @_python-pytest-method)
  (#set! tag python-pytest-method))

; decorated pytest functions
((module
  (decorated_definition
    (decorator)+ @_decorator
    definition: (function_definition
      name: (identifier) @run @_pytest_method_name
      (#match? @_pytest_method_name "^test_"))) @_python-pytest-method)
  (#set! tag python-pytest-method))

; pytest classes
(module
  (class_definition
    name: (identifier) @run @_pytest_class_name
    (#match? @_pytest_class_name "^Test"))
  (#set! tag python-pytest-class))

; decorated pytest classes
(module
  (decorated_definition
    (decorator)+ @_decorator
    definition: (class_definition
      name: (identifier) @run @_pytest_class_name
      (#match? @_pytest_class_name "^Test")))
  (#set! tag python-pytest-class))

; pytest class methods
(module
  (class_definition
    name: (identifier) @_pytest_class_name
    (#match? @_pytest_class_name "^Test")
    body: (block
      [
        (decorated_definition
          (decorator)+ @_decorator
          definition: (function_definition
            name: (identifier) @run @_pytest_method_name
            (#match? @_pytest_method_name "^test_")))
        (function_definition
          name: (identifier) @run @_pytest_method_name
          (#match? @_pytest_method_name "^test"))
      ] @_python-pytest-method)
    (#set! tag python-pytest-method)))

; decorated pytest class methods
(module
  (decorated_definition
    (decorator)+ @_decorator
    definition: (class_definition
      name: (identifier) @_pytest_class_name
      (#match? @_pytest_class_name "^Test")
      body: (block
        [
          (decorated_definition
            (decorator)+ @_decorator
            definition: (function_definition
              name: (identifier) @run @_pytest_method_name
              (#match? @_pytest_method_name "^test_")))
          (function_definition
            name: (identifier) @run @_pytest_method_name
            (#match? @_pytest_method_name "^test"))
        ] @_python-pytest-method)
      (#set! tag python-pytest-method))))

; module main method
(module
  (if_statement
    condition: (comparison_operator
      (identifier) @run @_lhs
      operators: "=="
      (string) @_rhs)
    (#eq? @_lhs "__name__")
    (#match? @_rhs "^[\"']__main__[\"']$")
    (#set! tag python-module-main-method)))

; module main method (membership test variant)
(module
  (if_statement
    condition: (comparison_operator
      (identifier) @run @_lhs
      operators: "in"
      (tuple
        (string) @_member
        (#match? @_member "^[\"']__main__[\"']$")))
    (#eq? @_lhs "__name__")
    (#set! tag python-module-main-method)))
