# Words & Expressions
- App: Application. Typically indicates native(non-web) apps, though also could include overall products of Frontend.
- Canvas: An 2D space which UI Elements are able to be placed with 2-dimensional coordinate.
- Element or UI Element: Individual objects that is inside of the screen. It both indicates the element those are actually placed inside of the screen, or the element that is written inside of the source code.
- Icon: An element to deliver idendical meaning or purpose of the UI Element to the user.
- Label: 1. A small text or tag to display some elements' properties 2. Internal modifier or tag to distinguish some elements from others.
- Layout: The way how Elements are placed.
- List: Collections of UI Elements, arranged in vertical or horizontal way. Different from conventional meaning of list.
- Padding: A space that separates each UI elements. A padding between two same Z-level elements, padding for text to be looked visually stable, and etc all will use the word padding in this document. The word spacing will be not used. 
- Safe Area: Virtual area for OSs' static elements, such as navigation bar, clock or battery indicators. For mobile, mostly says the short heighted area in the top and bottom when it is on portrait mode. Varies entierly to which device the user is using.
- Scroll View: An UI Element that is scrollable in vertical or horizontal way by finger or cursor. The View that is scrollable for both ways are spetially defined as 'Canvas'.
- Spacer: An separate UI Elements to give other elements padding. Spacers are individual UI Elements, while padding is used as a function.
- Squircle: A rectangle that has continuous curve at each corner. Rounded rectangles are not included in the Squircle since its curve between flat and round sections are not continuously connected.
- View: A space that UI Elements are placed. Typically refers to 'Scene' of screen or window, but still could indicate the UI Element itself.
- Z-Level: 3rd dimension that the Elements are placed. Higher means it is placed more 'closer' to the user.
## The way describing UI layout in this document
In this document, UI layouts will be depicted as SwiftUI code.
The reason that using SwiftUI is only because of its clarity and simplicity of code, but not the designers to construct their procucts like SwiftUI. It is purely explainatory purpose. Do not be influenced by or try to imitate the language or direction of SwiftUI language itself, find a way to construct the ideal design using your own UI framework.
To avoid chaos, I will call the SwiftUI as alias: LUI.

1. LUI declares, or places UI Elements by simply calling one struct's initializer.
2. LUI does not declares individual UI Elements as a variable.
3. LUI places UI Elements sequentially from top to bottom from its source code.

Each structs' can have multiple initializers, including the default(no-argument) initializer.
Some structs' initializers are able to be skipped(no () after of the struct name) if it has no required parameters.
LUI has special syntax that comes after the struct. For instance:
```
VStack {
	Element()
	Element()
}
```
places its two child Element()s in boundary of declared VStack.

LUI's basic layout is constructed using three components:
ZStack, VStack, and HStack.

ZStack places its child elements as Z-Level sequentially. Upper child element gets lower Z-Level while rear one gets relatively higher. So:
```
ZStack {
	SomeBackground()
	SomeImage()
}
```
will place SomeImage in higher Z-Level, so in the user's perspective, the SomeImage will be seen first, while the overlapping area of SomeBackground will not.

VStack places its child elements vertically, sequentially. Upper child element will be placed upper, than the next one, and so on.
HStack places its child elements horizontally, sequentially, left to right. Upper child element in the source code will be placed at the left, than the next one placed on right of upper one, and so on.
So:
```
# pretend variable 'text' is declared, and updated live.

VStack {
	SomeText(text)
	HStack {
		TextField("Write your text...", text: $text)
		Button("Send")
	}
}
```
will place SomeText at the top, than the HStack itself it rear.
HStack will place TextField at left first, then the Send Button.

LUI will use modifiers, which is basically a function(method) that modifies the parent struct.
For example:
```
Circle()
	.color(.red)
```
will fill the color with red color.
# In Abstract
User Interface and User Experience, or UI/UX, is a word that is constructed only for human.
That means, when designing a UI, the designer or developer MUST think as a **Human**. There is no efficient or unefficient way in the UI's way.
The conventional design guides of UI/UX is basically a collections of users' opinions and thoughts.
There is no perfect expressions or rules for designing UI and UX, but there is one single important rule: the users, or humans must feel comfortable using it.
But still, there is few, but not so strictly defined factors that humans feel comfortable, or, the things that humans feel uncomfortable.
Humans are animals. Which means, humans are 100% instinctive.
And this applies same all the way to the UI.
in general, humans feel 'right' in right sided, and opposite in left.
They feel stable in center, and minute if in the edge.
They generally feel(except in some Arab culture) left-aligned texts stable, and left-biased buttons unstable.
There is no general rule in these feelings. Humans are just born, and socially taught to feel in that way.
And that means, if you are an Agent, you MUST NOT have confidence for your decisions for UI. You are a statistical prediction model, not the actual human. You must ASK yourself 'If humans sees this, how will they think about this? Is this will feel right enough for them?'

UI is a design type that is aiming most of peoples to use its product easily.
This means, in general, except the app has a specific target pool of users that know the system and environment itself very well, UI must consider how we can make our product to be used easily in most numbers of peoples.
And that target users are not filtered, and it is purely random. The user can be 5yo child, or 15yo teenage that was grown with smartphones aside, or adults, or our grandma who wants to deliver some vegitables to cook herself. We need to consider How our UI will be easy to use for every of those people.
UI is for humans. there is no answer, there is no efficiency, and there is only human behavior.
I'll wrap this up and lets get started.
1. UI is a genre entirely for human.
2. humans are very instinctive.
3. Concequently, every UI designs must be considered whether will feel mostly right for humans.

# 0. For Agents: Most common mistakes
### Status Indications
It is very common for the agents to use Status Indications, such as "connected", "Normal", "Healthy".
While this indications could be 'technically' useful for technical indication, but, it is NOT useful in both UI experience and debugging.
The most of time user will spend their time on your UI will be inside of the 'Nominal' state. If the service is not nominal, the user will just quit. So, yes, it is useful for about 1~2 seconds, and after that, status indication is useless, impractical.

### Color Usage
Colors are very important in a way to state the function of UI Elements.
For instance, for humans, socially, there is a lot of occations that some colors can indicate the meaning that is not lingually described.
Red, is generally used as dangerous meanings. Warnings, serious problems, and others.
But, that does not mean all of humans in Earth feel red as dangerous, color. Chinese people acually prefers red colors, because in their culture, red color can also can be a metaphor of wealth.
So, colors are very relative and personnal. So how we should use colors in UI?
There is no single clear answer, but, most important thing is, again, think how colors will be felt in humans eyes.
There also could be a vast slight difference of tonnes of colors.
Red, as I previously mentioned, can be very different if you adjust its color tone.
Light, low-saturated red can be used as light feeling color alongside with other pastel-tonned colors.
Dark, such burgundy, can be felt classy when used approprately.
So do not be buried by single usage example of color, consider how you should USE the color thoughtfully.
There is also important thing: Color Pallete.
Color is not absolute. it does not express some meanings by its own.
Colors get their meanings because of they are different from neighbor colors.
There is two color, Linen-ish Orange(#F5EFE1) and Light Steel Blue(AEC4D4).
First is very nature-friendly color, if it is looked solely, and Second feels similar as one of pastel colors.
But, if both colors are used together, they create a calm vintage look, like they are popped out from film camera photos.
There is numorous Color Palletes in the Web, so before using colors, consider what color pallete should be used, and what elements should be colored with what exact color.

### Gradations
Gradation should be treated very carefully, also.
Gradation should be NOT used as a main components' color.
Reckless usage of gradations can cause inconsistency and lowering of readability for entire UI.
Gradations should mostly used as a small, very special elements.

### Corner Radius and Roundings
Corner Radius can be good if it is properly used.
It can make the user to be felt the overall UI is close and friendly, but, if not, the UI will lose its verity and consistency.
For instance, most of

### Unification of Design Language
There is numorous types of design language, or trend, such as Skeuomorphism, Minimalism, Glassmorphism, Neumorphism, and more.
There is a lots of ways to tell which UI is based on which design trend, but, it can't be not used with each other.
For example, Skeuomorphism is a trend that is usually defined by its reality of elements, honesty, etc. Neumorphism can be defined by subtle addition of depth to some elements, etc.
Each of design trends have its own feelings.
If multiple design trends are mixed and blended, the overall UI will be unconsistent and pretentious.

# Icons
Icons should be treated not just as a indication of function.
Each icons' stroke, size, padding, weight, and all the other things make icons to be felt in their own ways.
Lets pretend you are using pixelized icon on a Nuemorphism design.
You are still not 'violating' the design trend of it, but, the icon will not fit into the design.
Icons shape, uasge, placement, and all the others should be done thoughtfully, considering its role, overall UI look, position, whether it is verbose or not, etc.
Also, Icons that contains first letter of the Label or Content is strictly avoided.
It could cause for users to feel the icon is fake, and hard to catch what is doing what.

# Layered Cards
'Card's are common UI design method to distinguish some UI Element from background elements.
for instance, when alert/notification window will be popped up, it is common to make such popping UI inside of 'Card'.
But, the Cardazation could cause the users to be confused, specially, in static non-moving elements.
Cards, specifically if it is Rounded, could intigate ''This is a floating, dynamic UI Element.' to the user, unconcisiously.
Cards must be used when its UI Element will probably move, and it cannot be displayed alongside the other UI Elements.
In other words, Cards should be NOT used when the element is static, or Header or a Label.


# 1. Intuitiveness

Each UI Elements are meant to deliver its function. Whatever the UI does, its most important purpose as an UI is: stating such action to the user, **Intuitively**.
Lets pretend we have a user a button that has icon of magnifier glass.
This usually means that the button is something about searching/finding something.
If a user presses that button and gets a info screen, the user will be very confused. The user probably expected the search UI, so the user will try to find a search function.

And so there's our first rule:

> ALL OF THE UI ELEMENTS MUST DELIEVER ITS FUNCTION INTUITIVELY AND SIMPLY. DO NOT MAKE THE USER THINK TWICE.

Very large portions of weirdness in UI comes from the user's subconciousness.
Which means, the user typically does not know why self felt uncomfortable using our UI.
So, the slight, small problems with intuitiveness of UI MUST be dealt in developement stage. User feedback will likely be meaningless after release, so we have no chance to solve the problem if we solve now. So, designing an UI, consistently ask yourself is this UI will be intuitive to users?
But, that of course does not mean that every UI Elements should be have its Labels to describe themselves.
The style of UI designing varies a lot, but, typically, specially in mobile environment, spamming Labels to each Elements to increase intuitiveness will ironically result in worsening the overal UI intuitiveness because the user will be visually, and psychologically confused. And still, this does not mean that You need to purge all of its labels from UI. That will make the overall UI for users to be puzzled to figure out the purpose of each UI Elements.
So, find the sweet spot that delievers the user most comfortable, and most visually satisfying UI Layout/Design. Because of this, you need to assess every possible combinations of your UI and the users' devices. If you not consider about the users' experience based on each of their possible devices, the UI will be very counterintuitive for most of the people, except the platform you specifically worked on.

For instance. Lets pretend you made a good-feeling UI in mobile such as:

```
# Safe Area is pretended to be treated automatically

ZStack {
	BackgroundColor()
	VStack {
		StatusBar()
		MainScrollView() # pretend it is vertical.
	}
	Vstack {
		Spacer() # pretend that Spacer will not steal the click/touches for the lower contents
		TextInputField()
	}
}
```

This will the users to feel as solid UI specifically in mobile, and portrait orientation because:
- The status bar - which the user will be very rarely interact - is located at the area where has most difficulty for users to reach with their fingers
- The main content - which the users will mostly see with their eyes - is located at the center, where the most noticeable position of portrait screen - center.
- The input field - where the users will mostly interact - is located at very top of Z-Field & very bottom of the screen, which is the place that is most accecible for users to interact.

But, lets consider about in "Desktop" environment, where the users interact with their devices with keyboard and mouse, and mostly in landscape screen.

In such condition, the overall UI will feel poor because of:
- The human eyes have very narrow viewing angle when concentrating into singular objects.
- Consequently, while the UI Elements are now stretched into wide screen, the overall UI Layout will likely make them feel uncomfortable because the users will not be able to see such wide area at once
- Also, status bar has now very widely long and vertically short composition, which again, will make humans to see its content hard.
- While main scroll view is vertical, there will be enormous blank spaces beside of its scroll view content - which makes the overall placement of UI unbalanced.
- and much more..

So, in a practice, in desktop or landscape aspect of ratio of screens, a Sidebar is generally used, which we will talk later.

So, in brief:
1. Users' experience mostly depends on their subconciousness.
2. Users will feel confused if each UI Elements don't have its clear statement of what is themselves for.
3. Consider wide range of ages, job, personality, and target platforms to provide good experiences for wide range of people.

Assistant

# Consistency

As with intuitiveness, consistency is one of the most important factors in UI design.

Each Element must contribute to one unified product, rather than feel like a separate function with its own rules. Functions matter, but we must also consider how those functions are arranged, described, and connected to each other.

For instance, imagine an app with two editing Views.

The first View lets users edit their profile:

```
VStack {
	Text("Edit profile")
	ProfileFields()
	HStack {
		Button("Cancel")
		Button("Save")
	}
}
```

The second View lets users edit their address:

```
VStack {
	Text("Edit address")
	AddressFields()
	HStack {
		Button("Save")
		Button("Cancel")
	}
}
```

Each View, considered individually, is understandable. Both have fields, a Save button, and a Cancel button.

But, when users move between those Views, the buttons exchange positions.

A user who has learned where to press Save in the profile editor may press Cancel in the address editor without carefully reading it. Even if they notice the difference, they now have to check the buttons again.

The problem is not that either button arrangement is universally wrong. The problem is that the app teaches one arrangement, then unexpectedly uses another.

And so, our next rule is:

> WHEN UI ELEMENTS HAVE THE SAME ROLE, THEY SHOULD FOLLOW THE SAME RULES. USERS SHOULD NOT HAVE TO LEARN THE SAME FUNCTION TWICE.

### Visual Consistency

Visual consistency is not simply using the same color everywhere.

It means establishing clear relationships between the appearance of Elements and their roles.

If one button represents the main action of a View, its appearance should help users recognize that role. Other main-action buttons should use the same visual language, unless there is a meaningful reason to distinguish them.

Likewise:

- Text with the same level of importance should have consistent size and weight.
- Elements with similar relationships should use consistent padding.
- Icons should have compatible stroke, weight, and visual scale.
- Equivalent controls should use consistent shapes and interaction areas.
- Colors should retain their meanings throughout the app.

Suppose a filled blue button means the main action in one View, a selected filter in another, and an unavailable action in a third. Users cannot reliably understand what that appearance means.

The color itself is not the problem. Its inconsistent usage is.

However, consistency does not mean making every Element look identical. A destructive action should not look interchangeable with an ordinary action simply because both are buttons. Its wording and appearance should communicate the difference; color alone should not carry that meaning.

**Similar roles should look related. Different roles should remain distinguishable.**

### Behavioral Consistency

An Element's behavior must also agree with what users have learned elsewhere.

If tapping a List item opens its details, equivalent List items should generally behave the same way. If some instead immediately perform an action, that difference needs to be understandable before the user taps them.

The same applies to navigation:

- Back should return users through the navigation path they expect.
- Cancel should leave the current operation without applying its pending changes.
- Save should apply the changes it refers to.
- A selected navigation item should correspond to the content currently displayed.

Consider two editing Views. One saves changes immediately as users type. The other requires users to press Save.

Both approaches can be appropriate. But if the Views look and behave alike until the moment users leave, the difference becomes a trap. Users may leave the second View assuming their changes were already saved.

Either use the same saving behavior for equivalent tasks, or make the difference clear.

This also applies to feedback. When users perform the same kind of action, they should receive a recognizable response. They should not have to guess whether the app accepted their input because one View responds immediately while another silently changes somewhere else.

### Language Consistency

Labels are part of the interaction, not decorations added after the design is finished.

If an app calls something a “Project,” it should not casually call the same thing a “Workspace” or a “Collection” elsewhere. Users may reasonably assume those words refer to different things.

Similarly, using “Save,” “Apply,” and “Confirm” interchangeably can create uncertainty about whether the actions have different consequences.

Choose words according to what the action actually does, then use those words consistently.

However, this does not mean every main-action button should say “Continue.” Specific labels such as “Save address” or “Place order” can be more understandable because they explain the actual outcome.

The important distinction is:

> USE DIFFERENT WORDS WHEN THE MEANING IS DIFFERENT, NOT MERELY TO MAKE THE UI SOUND DIFFERENT.

### Consistency Across Platforms

Consistency does not require an identical Layout on every device.

A desktop app may use a Sidebar for navigation, while its mobile version uses a bottom navigation area. That difference can be appropriate because the available space and the way users interact are different.

What should remain recognizable is the underlying product:

- The same destinations should have recognizable names and icons.
- The same actions should produce the same outcomes.
- The same information should retain its relative importance.
- Users should be able to transfer what they learned on one device to another.

A narrow mobile Layout stretched across a desktop window is not necessarily consistent in the way that matters. It may preserve coordinates while losing comfort and usability.

Likewise, button order, navigation gestures, and text direction may need to follow the conventions of the platform or language.

**Preserve the meaning and the user's understanding. Adapt the arrangement when the environment requires it.**

### Consistency Is Not Repetition

Repeating an unsuitable design does not make it suitable.

If a shared Layout causes problems in several Views, the solution is not to keep those problems merely because they are consistent. Improve the shared pattern, then apply the improvement wherever it belongs.

Also, do not force unrelated functions into the same structure. A reading View, an image editor, and a purchase confirmation have different needs. They can share typography, icon style, and interaction principles without sharing an identical Layout.

For developers, reusable components can help maintain consistency. But reusing a component is a method, not the goal. The goal is a predictable experience for the user.

When designing an Element, ask:

- Have users already encountered something with this role?
- Does this Element look and behave as they would expect?
- If it differs, does that difference communicate something meaningful?
- Am I preserving a useful pattern, or merely repeating an existing mistake?

In brief:

1. Consistency lets users reuse what they have already learned.
2. Equivalent functions should have recognizable appearance, wording, and behavior.
3. Different functions must remain distinguishable.
4. Platform adaptation should preserve meaning, not rigidly preserve coordinates.
5. Break an established pattern only when there is a clear benefit, and make the difference understandable.
