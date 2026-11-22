#set page(paper: "a4", margin: 20mm)

#let bake-customer = _ferris_data.at("customer", default: (:))
#let bake-items = _ferris_data.at("items", default: ())
#let bake-total = if "total" in _ferris_data {
  _ferris_data.at("total")
} else {
  bake-items.fold(0, (acc, it) => acc + it.at("rate", default: 0) * it.at("qty", default: 0))
}

#let fmt-customer(c) = if type(c) == str { c } else { c.at("name", default: "") }

#text(size: 18pt, weight: "bold")[Invoice]

#v(0.5cm)

#align(left)[
  #text(size: 11pt)[Customer: #fmt-customer(bake-customer)]
]

#v(0.5cm)

#for it in bake-items [
  #text(it.at("description", default: ""))
  #h(1fr)
  #it.at("qty", default: 0)
  #h(1fr)
  #it.at("rate", default: 0)
  #v(0.2cm)
]

#v(0.5cm)

#align(right)[
  #text(size: 12pt, weight: "bold")[Total: #bake-total]
]
